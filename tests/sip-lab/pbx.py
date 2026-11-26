#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""sip-lab PBX — minimal, deterministic SIP registrar + proxy for loopback testing.

Standard library only (Python 3.10+). UDP/IPv4, bound to 127.0.0.1 by default.

What it does
------------
* REGISTER / unregister (``Expires: 0``) with HTTP Digest auth (MD5, qop=auth).
  The challenge realm is fixed (default ``sip-lab``); test accounts live in
  ``accounts.json`` (throwaway, loopback-only, never real credentials).
* INVITE proxying: local ``100 Trying``, relaying of ``180``/``200`` (the SDP
  offer/answer passes through untouched and is logged), ``Record-Route`` added
  so ACK / BYE / re-INVITE flow through the PBX, ``CANCEL`` reuses the INVITE
  transaction branch, ``486 Busy Here`` while a line is ringing/active.
* ``404`` for unregistered/unknown targets, ``405`` for unknown methods,
  ``483`` on Max-Forwards exhaustion, ``200`` for ``OPTIONS`` keepalives.

What it deliberately does NOT do (documented lab limits): TCP/TLS, SRTP,
NAT traversal, forking, presence/MESSAGE handling, or any media handling —
RTP flows directly between the endpoints (the PBX is signaling-only).

Usage:  python3 pbx.py [--host 127.0.0.1] [--port 5060] [--accounts FILE]
"""
from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import re
import secrets
import signal
import socket
import sys
import time
from typing import Dict, Optional, Tuple

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from siplib import (  # noqa: E402
    build_msg,
    check_digest,
    hget,
    parse_digest,
    parse_msg,
    sdp_summary,
    uri_dest,
    uri_user,
    via_branch,
)

LAB = os.path.dirname(os.path.abspath(__file__))

ALLOW = ("INVITE, ACK, BYE, CANCEL, REGISTER, OPTIONS, UPDATE, INFO, "
         "REFER, NOTIFY, SUBSCRIBE, MESSAGE")
SERVER_HDR = "sip-lab-pbx/0.1"
REASONS = {
    100: "Trying", 180: "Ringing", 183: "Session Progress", 200: "OK",
    400: "Bad Request", 401: "Unauthorized", 403: "Forbidden",
    404: "Not Found", 405: "Method Not Allowed", 408: "Request Timeout",
    481: "Call/Transaction Does Not Exist", 483: "Too Many Hops",
    486: "Busy Here", 487: "Request Terminated", 500: "Server Error",
}
NONCE_TTL = 600.0        # seconds a digest nonce stays valid
TXN_TTL = 180.0          # seconds we keep forwarding state for a transaction
MAX_EXPIRES = 86400      # cap on registration lifetime

log = logging.getLogger("pbx")


def new_branch() -> str:
    return "z9hG4bK" + secrets.token_hex(8)


def first_via(msg: Dict) -> str:
    vias = [v for n, v in msg["headers"] if n.lower() == "via"]
    return vias[0] if vias else ""


def cseq_parts(msg: Dict) -> Tuple[str, str]:
    parts = (hget(msg, "cseq") or "").split()
    num = parts[0] if parts else ""
    method = parts[1].upper() if len(parts) > 1 else ""
    return num, method


def split_header_values(v: str):
    """Split a comma-separated header value list (Route/Record-Route)."""
    return [s.strip() for s in v.split(",") if s.strip()]


class PBX(asyncio.DatagramProtocol):
    def __init__(self, host: str, port: int, accounts: Dict[str, str],
                 realm: str, advertise: str, auth_invite: bool):
        self.host = host
        self.port = port
        self.accounts = accounts
        self.realm = realm
        self.advertise = advertise
        self.auth_invite = auth_invite
        self.transport: Optional[asyncio.DatagramTransport] = None
        self.my_ip = host
        self.my_port = port
        # aor user -> {"contact", "dest": (ip, port), "expires": ts, "src"}
        self.bindings: Dict[str, Dict] = {}
        # set of aor users with a ringing/active call (line-busy)
        self.busy: set = set()
        # call-id -> {"caller", "callee", "state", "t"}
        self.calls: Dict[str, Dict] = {}
        # (call-id, cseq-num, orig-branch, method) -> {bytes, our_branch, dest, expires}
        self.seen: Dict[tuple, Dict] = {}
        # (our-branch, cseq-method) -> {orig_addr, expires, call-id}
        self.fwd: Dict[tuple, Dict] = {}
        # nonce -> ts
        self.nonces: Dict[str, float] = {}
        self.stats = {"req": 0, "resp": 0, "fwd": 0, "drop": 0}
        self._tasks = []

    # ---------------- lifecycle ----------------

    def connection_made(self, transport):
        self.transport = transport
        sock = transport.get_extra_info("sockname") or (self.host, self.port)
        self.my_ip, self.my_port = sock[0], sock[1]
        log.info("listening on udp://%s:%d (advertise=%s) realm=%s accounts=%d "
                 "auth_invite=%s", self.my_ip, self.my_port, self.advertise,
                 self.realm, len(self.accounts), self.auth_invite)
        task = asyncio.get_event_loop().create_task(self._cleanup_loop())
        self._tasks.append(task)

    def error_received(self, exc):
        log.error("transport error: %r", exc)

    def connection_lost(self, exc):
        for t in self._tasks:
            t.cancel()

    # ---------------- entry points ----------------

    def datagram_received(self, data: bytes, addr):
        msg = parse_msg(data)
        if msg is None:
            self.stats["drop"] += 1
            log.warning("DROP unparseable datagram (%d bytes) from %s:%d",
                        len(data), addr[0], addr[1])
            return
        try:
            if msg["kind"] == "req":
                self.stats["req"] += 1
                self.on_request(msg, addr)
            else:
                self.stats["resp"] += 1
                self.on_response(msg, addr)
        except Exception:  # never let one bad message kill the loop
            log.exception("error handling message from %s:%d", addr[0], addr[1])

    # ---------------- request routing ----------------

    def on_request(self, msg: Dict, addr):
        method = msg["method"]
        uri = msg["uri"]
        log.info("RX %-9s %s from %s:%d%s", method, uri, addr[0], addr[1],
                 self._sdp_note(msg))
        if method == "REGISTER":
            return self.on_register(msg, addr)
        if method == "OPTIONS":
            return self.respond(msg, addr, 200,
                                extra=[("Allow", ALLOW), ("Server", SERVER_HDR)])
        if method == "CANCEL":
            return self.on_cancel(msg, addr)
        if method == "INVITE" and not self._to_has_tag(msg):
            return self.on_new_invite(msg, addr)
        if method in ("ACK", "BYE", "INVITE", "UPDATE", "INFO", "REFER",
                      "NOTIFY", "SUBSCRIBE", "MESSAGE"):
            return self.forward_generic(msg, addr, method)
        self.respond(msg, addr, 405, extra=[("Allow", ALLOW)], to_tag=True)

    # ---------------- REGISTER ----------------

    def challenge(self, msg: Dict, addr, reason: str = ""):
        nonce = secrets.token_hex(16)
        self.nonces[nonce] = time.time()
        hdr = 'Digest realm="%s", nonce="%s", algorithm=MD5, qop="auth"' % (
            self.realm, nonce)
        if reason:
            log.info("AUTH challenge (%s) aor=%s -> %s:%d", reason,
                     uri_user(hget(msg, "to") or "") or "?", addr[0], addr[1])
        self.respond(msg, addr, 401, extra=[("WWW-Authenticate", hdr)])

    def on_register(self, msg: Dict, addr):
        to = hget(msg, "to") or ""
        aor = uri_user(to) or uri_user(hget(msg, "from") or "")
        if not aor:
            return self.respond(msg, addr, 400, to_tag=False)
        params = parse_digest(hget(msg, "authorization"))
        if not params:
            return self.challenge(msg, addr, "no credentials")
        nonce = params.get("nonce", "")
        if nonce not in self.nonces or time.time() - self.nonces[nonce] > NONCE_TTL:
            return self.challenge(msg, addr, "stale nonce")
        ok, why = check_digest(params, "REGISTER", msg["uri"],
                               self.accounts, self.realm)
        if not ok:
            return self.challenge(msg, addr, "auth failed: %s" % why)
        if params.get("username") != aor:
            log.info("AUTH fail: username %s != aor %s", params.get("username"), aor)
            return self.respond(msg, addr, 403)
        log.info("AUTH ok user=%s from %s:%d", aor, addr[0], addr[1])

        contact = hget(msg, "contact")
        exp = None
        eh = hget(msg, "expires")
        if eh and eh.strip().isdigit():
            exp = int(eh.strip())
        if exp is None and contact:
            m = re.search(r"expires=(\d+)", contact, re.I)
            exp = int(m.group(1)) if m else None
        if exp is None:
            exp = 3600
        exp = max(0, min(exp, MAX_EXPIRES))

        # wildcard unregistration
        if contact and contact.strip().strip("<>") == "*":
            if exp == 0:
                old = self.bindings.pop(aor, None)
                log.info("UNREG aor=%s (wildcard)%s", aor,
                         " contact=%s" % old["contact"] if old else "")
                return self.respond(msg, addr, 200,
                                    extra=[("Expires", "0"), ("Contact", "*")])
            return self.respond(msg, addr, 400,
                                extra=[("Warning",
                                        '300 sip-lab "Contact * requires Expires 0"')])

        if exp == 0:
            old = self.bindings.pop(aor, None)
            extra = [("Expires", "0")]
            if old:
                extra.append(("Contact", "<%s>;expires=0" % old["contact"]))
            log.info("UNREG aor=%s src=%s:%d", aor, addr[0], addr[1])
            return self.respond(msg, addr, 200, extra=extra)

        curi = self._contact_uri(contact) or "sip:%s@%s:%d" % (aor, addr[0], addr[1])
        host, port = uri_dest(curi, default_port=None)
        host = host or addr[0]
        port = port or addr[1]
        self.bindings[aor] = {
            "contact": curi, "dest": (host, port),
            "expires": time.time() + exp, "src": addr,
        }
        log.info("REG   aor=%s contact=%s expires=%d dest=%s:%d",
                 aor, curi, exp, host, port)
        self.respond(msg, addr, 200, extra=[
            ("Contact", "<%s>;expires=%d" % (curi, exp)),
            ("Expires", str(exp)),
            ("Server", SERVER_HDR),
        ])

    # ---------------- INVITE ----------------

    def on_new_invite(self, msg: Dict, addr):
        cid = hget(msg, "call-id") or ""
        num, _ = cseq_parts(msg)
        orig_branch = via_branch(first_via(msg))

        if self.auth_invite:
            params = parse_digest(hget(msg, "authorization"))
            ok, why = check_digest(params or {}, "INVITE", msg["uri"],
                                   self.accounts, self.realm)
            if not ok:
                return self.challenge(msg, addr, "invite auth: %s" % why)

        user = uri_user(msg["uri"])
        callee = None
        if user:
            b = self.bindings.get(user)
            if b and b["expires"] <= time.time():
                del self.bindings[user]
                b = None
            if not b:
                log.info("INVITE %s -> 404 (not registered)", user)
                return self.respond(msg, addr, 404, to_tag=True)
            if user in self.busy:
                log.info("INVITE %s -> 486 (line busy)", user)
                return self.respond(msg, addr, 486, to_tag=True)
            dest = b["dest"]
            callee = user
        else:
            dest = uri_dest(msg["uri"])
            if not dest[0]:
                log.info("INVITE %s -> 404 (no target)", msg["uri"])
                return self.respond(msg, addr, 404, to_tag=True)

        caller = uri_user(hget(msg, "from") or "") or "?"
        key = (cid, num, orig_branch, "INVITE")
        if key in self.seen:  # retransmission: resend the same forwarded bytes
            self.transport.sendto(self.seen[key]["bytes"], dest)
            log.info("FWD  INVITE (retransmission) -> %s:%d", dest[0], dest[1])
            return

        self.respond(msg, addr, 100)  # 100 Trying, before forwarding
        our_branch = new_branch()
        fwd = self.build_forward(msg, dest, our_branch, record_route=True)
        now = time.time()
        self.seen[key] = {"bytes": fwd, "our_branch": our_branch,
                          "dest": dest, "expires": now + TXN_TTL}
        self.fwd[(our_branch, "INVITE")] = {
            "orig_addr": addr, "expires": now + TXN_TTL, "call_id": cid}
        if callee:
            self.busy.add(callee)
        self.calls[cid] = {"caller": caller, "callee": callee,
                           "state": "proceeding", "t": now}
        self.stats["fwd"] += 1
        self.transport.sendto(fwd, dest)
        log.info("FWD  INVITE %s -> %s:%d (branch=%s, call-id=%s)%s",
                 caller + "=>" + (callee or msg["uri"]), dest[0], dest[1],
                 our_branch, cid[:16], self._sdp_note(msg))

    def on_cancel(self, msg: Dict, addr):
        cid = hget(msg, "call-id") or ""
        num, _ = cseq_parts(msg)
        orig_branch = via_branch(first_via(msg))
        inv = self.seen.get((cid, num, orig_branch, "INVITE"))
        if not inv:
            log.info("CANCEL -> 481 (no matching INVITE transaction)")
            return self.respond(msg, addr, 481)
        key = (cid, num, orig_branch, "CANCEL")
        if key in self.seen:
            self.transport.sendto(self.seen[key]["bytes"], inv["dest"])
            return
        our_branch = inv["our_branch"]
        dest = inv["dest"]
        fwd = self.build_forward(msg, dest, our_branch)  # same branch = same txn
        now = time.time()
        self.seen[key] = {"bytes": fwd, "our_branch": our_branch,
                          "dest": dest, "expires": now + TXN_TTL}
        self.fwd[(our_branch, "CANCEL")] = {
            "orig_addr": addr, "expires": now + TXN_TTL, "call_id": cid}
        self.stats["fwd"] += 1
        self.transport.sendto(fwd, dest)
        log.info("FWD  CANCEL -> %s:%d (reuses INVITE branch %s)",
                 dest[0], dest[1], our_branch)

    def forward_generic(self, msg: Dict, addr, method: str):
        cid = hget(msg, "call-id") or ""
        num, _ = cseq_parts(msg)
        orig_branch = via_branch(first_via(msg))
        key = (cid, num, orig_branch, method)
        if key in self.seen:
            self.transport.sendto(self.seen[key]["bytes"], self.seen[key]["dest"])
            log.info("FWD  %s (retransmission) -> %s:%d", method,
                     self.seen[key]["dest"][0], self.seen[key]["dest"][1])
            return
        mf = hget(msg, "max-forwards")
        if mf is not None and mf.strip().isdigit() and int(mf.strip()) == 0:
            return self.respond(msg, addr, 483)
        dest = uri_dest(msg["uri"])
        if not dest[0] or self.is_us(dest):
            # Loose routing: R-URI is us only in strict-route clients; try Route.
            route = hget(msg, "route")
            if route and method != "ACK":
                dest = uri_dest(split_header_values(route)[0])
            else:
                if method == "ACK":
                    log.info("ACK  consumed locally (response we generated)")
                    return
                log.info("%s -> 481 (R-URI is this PBX)", method)
                return self.respond(msg, addr, 481, to_tag=True)
        our_branch = new_branch()
        fwd = self.build_forward(msg, dest, our_branch)
        now = time.time()
        self.seen[key] = {"bytes": fwd, "our_branch": our_branch,
                          "dest": dest, "expires": now + TXN_TTL}
        self.fwd[(our_branch, method)] = {
            "orig_addr": addr, "expires": now + TXN_TTL, "call_id": cid}
        self.stats["fwd"] += 1
        self.transport.sendto(fwd, dest)
        log.info("FWD  %-7s -> %s:%d (branch=%s)", method, dest[0], dest[1],
                 our_branch)
        if method == "BYE":
            c = self.calls.pop(cid, None)
            if c:
                if c.get("callee"):
                    self.busy.discard(c["callee"])
                log.info("CALL ended (BYE) %s => %s", c.get("caller"),
                         c.get("callee") or "external")

    # ---------------- responses ----------------

    def on_response(self, msg: Dict, addr):
        branch = via_branch(first_via(msg))
        _, cmethod = cseq_parts(msg)
        txn = self.fwd.get((branch, cmethod))
        status = msg["status"]
        if not txn:
            self.stats["drop"] += 1
            log.info("DROP response %d %s (no txn for branch=%s) from %s:%d",
                     status, msg["reason"], branch, addr[0], addr[1])
            return
        # strip our own topmost Via, relay the rest to the original requester
        new_headers = []
        stripped = False
        for n, v in msg["headers"]:
            if not stripped and n.lower() == "via" and branch and branch in v:
                segs = split_header_values(v)
                keep = [s for s in segs if via_branch(s) != branch]
                stripped = True
                if keep:
                    new_headers.append((n, ", ".join(keep)))
                continue
            new_headers.append((n, v))
        if not stripped:
            self.stats["drop"] += 1
            log.warning("DROP response %d (our Via not found) from %s:%d",
                        status, addr[0], addr[1])
            return
        data = build_msg(msg["start"], new_headers, msg["body"])
        dst = txn["orig_addr"]
        self.transport.sendto(data, dst)
        log.info("TX  %3d %-22s -> %s:%d%s", status, msg["reason"],
                 dst[0], dst[1], self._sdp_note(msg))
        if cmethod == "INVITE" and status >= 200:
            cid = hget(msg, "call-id") or ""
            c = self.calls.get(cid)
            if status >= 300:
                if c:
                    if c.get("callee"):
                        self.busy.discard(c["callee"])
                    del self.calls[cid]
                    log.info("CALL failed (%d %s) %s => %s", status,
                             msg["reason"], c.get("caller"),
                             c.get("callee") or "external")
            elif c:
                c["state"] = "confirmed"
                log.info("CALL answered (200 OK) %s => %s%s", c.get("caller"),
                         c.get("callee") or "external", self._sdp_note(msg))

    # ---------------- message building ----------------

    def respond(self, req: Dict, addr, status: int, extra=(), body: bytes = b"",
                to_tag: bool = False):
        reason = REASONS.get(status, "OK")
        vias = [(n, v) for n, v in req["headers"] if n.lower() == "via"]
        frm = [(n, v) for n, v in req["headers"] if n.lower() == "from"]
        cid_h = [(n, v) for n, v in req["headers"] if n.lower() == "call-id"]
        cseq_h = [(n, v) for n, v in req["headers"] if n.lower() == "cseq"]
        to = hget(req, "to") or ""
        if to_tag and not re.search(r"[;\s]tag=", to, re.I):
            to += ";tag=" + secrets.token_hex(4)
        headers = vias + frm + [("To", to)] + cid_h + cseq_h + list(extra)
        data = build_msg("SIP/2.0 %d %s" % (status, reason), headers, body)
        self.transport.sendto(data, addr)
        note = ""
        if body:
            note = " " + ", ".join(sdp_summary(body))
        log.info("TX  %3d %-22s -> %s:%d%s", status, reason, addr[0], addr[1], note)

    def build_forward(self, msg: Dict, dest, branch: str,
                      record_route: bool = False) -> bytes:
        hop = self.advertise or self.my_ip
        headers = [("Via", "SIP/2.0/UDP %s:%d;branch=%s;rport"
                   % (hop, self.my_port, branch))]
        for n, v in msg["headers"]:
            ln = n.lower()
            if ln == "via":
                headers.append((n, v))
            elif ln == "max-forwards":
                try:
                    mv = str(max(0, int(v.strip()) - 1))
                except ValueError:
                    mv = "69"
                headers.append((n, mv))
            elif ln == "route":
                keep = [s for s in split_header_values(v)
                        if not self.is_us(uri_dest(s))]
                if keep:
                    headers.append((n, ", ".join(keep)))
            elif ln == "content-length":
                continue
            else:
                headers.append((n, v))
        if record_route:
            headers.append(("Record-Route",
                            "<sip:%s:%d;transport=udp;lr>"
                            % (hop, self.my_port)))
        return build_msg(msg["start"], headers, msg["body"])

    # ---------------- misc ----------------

    def is_us(self, dest) -> bool:
        if not dest or dest[1] != self.my_port:
            return False
        return dest[0] in {self.advertise, self.my_ip, "0.0.0.0"}

    @staticmethod
    def _contact_uri(contact: Optional[str]) -> Optional[str]:
        if not contact:
            return None
        c = contact.strip()
        if "<" in c and ">" in c:
            c = c[c.index("<") + 1:c.index(">")]
        else:
            c = split_header_values(c)[0]
        return c.strip() or None

    @staticmethod
    def _to_has_tag(msg: Dict) -> bool:
        return bool(re.search(r"[;\s]tag=", hget(msg, "to") or "", re.I))

    @staticmethod
    def _sdp_note(msg: Dict) -> str:
        body = msg.get("body") or b""
        if not body:
            return ""
        lines = sdp_summary(body)
        return (" | SDP: " + ", ".join(lines)) if lines else ""

    async def _cleanup_loop(self):
        while True:
            await asyncio.sleep(5)
            now = time.time()
            for aor in [a for a, b in self.bindings.items()
                        if b["expires"] <= now]:
                del self.bindings[aor]
                log.info("REG expired aor=%s", aor)
            for n in [n for n, t in self.nonces.items() if now - t > NONCE_TTL]:
                del self.nonces[n]
            for k in [k for k, e in self.seen.items() if e["expires"] <= now]:
                del self.seen[k]
            for k in [k for k, e in self.fwd.items() if e["expires"] <= now]:
                del self.fwd[k]
            for cid in [c for c, v in self.calls.items()
                        if (v["state"] == "proceeding"
                            and now - v["t"] > 120)
                        or (v["state"] == "confirmed"
                            and now - v["t"] > 600)]:
                c = self.calls.pop(cid)
                if c.get("callee"):
                    self.busy.discard(c["callee"])
                log.warning("CALL reaped (no BYE/final response) %s => %s after "
                            "%ds", c.get("caller"), c.get("callee"),
                            int(now - c["t"]))


def load_accounts(path: str):
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)
    users = data.get("users", data)
    accounts = {}
    for u, v in users.items():
        if u.startswith("_") or not isinstance(v, (str, dict)):
            continue
        accounts[u] = v["password"] if isinstance(v, dict) else v
    realm = data.get("realm", "sip-lab")
    return accounts, realm


def detect_advertise(host: str) -> str:
    if host not in ("0.0.0.0", "::"):
        return host
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("8.8.8.8", 53))  # UDP connect: route lookup only, no packets
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


def main() -> int:
    ap = argparse.ArgumentParser(description="sip-lab PBX (registrar + proxy)")
    ap.add_argument("--host", default="127.0.0.1",
                    help="bind address (default: loopback only)")
    ap.add_argument("--port", type=int, default=5060)
    ap.add_argument("--accounts", default=os.path.join(LAB, "accounts.json"))
    ap.add_argument("--realm", default=None,
                    help="digest realm (default: realm from accounts.json)")
    ap.add_argument("--advertise", default=None,
                    help="IP advertised in Via/Record-Route "
                         "(default: --host, auto-detected if 0.0.0.0)")
    ap.add_argument("--auth-invite", action="store_true",
                    help="require digest auth for INVITE too (off by default)")
    ap.add_argument("--log-file", default=os.path.join(LAB, "logs", "pbx.log"))
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    os.makedirs(os.path.dirname(args.log_file), exist_ok=True)
    fmt = logging.Formatter("%(asctime)s.%(msecs)03d %(levelname)-5s %(message)s",
                            "%H:%M:%S")
    root = logging.getLogger()
    root.setLevel(logging.DEBUG if args.verbose else logging.INFO)
    sh = logging.StreamHandler(sys.stdout)
    sh.setFormatter(fmt)
    fh = logging.FileHandler(args.log_file, mode="w", encoding="utf-8")
    fh.setFormatter(fmt)
    root.handlers = [sh, fh]

    accounts, realm = load_accounts(args.accounts)
    if args.realm:
        realm = args.realm
    advertise = args.advertise or detect_advertise(args.host)

    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    transport, _ = loop.run_until_complete(loop.create_datagram_endpoint(
        lambda: PBX(args.host, args.port, accounts, realm, advertise,
                    args.auth_invite),
        local_addr=(args.host, args.port)))
    pbx: PBX = transport.get_extra_info("protocol")

    stop = asyncio.Event()
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, stop.set)

    async def run():
        await stop.wait()
        log.info("shutdown requested — stats: %s", pbx.stats)
        transport.close()

    try:
        loop.run_until_complete(run())
    finally:
        loop.close()
    log.info("bye")
    return 0


if __name__ == "__main__":
    sys.exit(main())
