#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Minimal scripted SIP UA over UDP — the test harness for the sip-lab PBX.

One class (UA) that can REGISTER (with digest), send INVITE with an SDP
offer, answer an INVITE (180/200 with SDP answer), ACK, BYE, and CANCEL.
Blocking sockets with timeouts: deterministic, single-threaded, stdlib only.
"""
from __future__ import annotations

import os
import secrets
import socket
import sys
import time
from typing import Callable, Dict, List, Optional, Tuple

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from siplib import (  # noqa: E402
    build_msg, digest_response, hget, parse_digest, parse_msg, uri_dest,
    uri_user, via_branch,
)

PBX = ("127.0.0.1", 5060)


class SipTimeout(Exception):
    pass


class UA:
    """A tiny blocking SIP user agent bound to an ephemeral UDP port."""

    def __init__(self, user: str, password: str, realm: str = "sip-lab",
                 pbx: Tuple[str, int] = PBX, label: str = ""):
        self.user = user
        self.password = password
        self.realm = realm
        self.pbx = pbx
        self.label = label or user
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.settimeout(0.2)
        self.port = self.sock.getsockname()[1]
        self.from_tag = secrets.token_hex(4)
        self.cseq = 0
        self.log: List[str] = []          # ordered trace of what we saw
        self.dialogs: Dict[str, Dict] = {}  # call-id -> dialog state

    # ---------------- low level ----------------

    def _send(self, data: bytes, dest=None):
        self.sock.sendto(data, dest or self.pbx)

    def send_request(self, method: str, uri: str, headers: List[tuple],
                     body: bytes = b"", dest=None) -> str:
        self.cseq += 1
        branch = "z9hG4bK" + secrets.token_hex(8)
        all_headers = [
            ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=%s;rport"
             % (self.port, branch)),
        ] + headers + [("CSeq", "%d %s" % (self.cseq, method))]
        if body:
            all_headers.append(("Content-Type", "application/sdp"))
        data = build_msg("%s %s SIP/2.0" % (method, uri), all_headers, body)
        self._send(data, dest)
        self.log.append("TX %s %s" % (method, uri))
        return branch

    @staticmethod
    def _desc(msg: Dict) -> str:
        return ("%s %s" % (msg["method"], msg["uri"]) if msg["kind"] == "req"
                else "%d %s" % (msg["status"], msg["reason"]))

    def recv_matching(self, pred: Callable[[Dict], bool], timeout: float,
                      what: str) -> Dict:
        """Drain the buffer, then the socket, until `pred(msg)` matches.

        Non-matching messages are buffered (never dropped) so a later wait
        can still see them. Raises SipTimeout with the recent trace.
        """
        if not hasattr(self, "_buf"):
            self._buf: List[bytes] = []
        deadline = time.time() + timeout
        while True:
            # scan the buffer WITHOUT dropping non-matching messages: they
            # may be the answer to a later wait (e.g. 487 after 200-CANCEL)
            for i, data in enumerate(self._buf):
                msg = parse_msg(data)
                if msg is None:
                    self._buf.pop(i)
                    break
                if pred(msg):
                    self._buf.pop(i)
                    self.log.append("RX(buf) " + self._desc(msg))
                    return msg
            remaining = deadline - time.time()
            if remaining <= 0:
                raise SipTimeout("timeout waiting for %s (trace: %s)"
                                 % (what, self.log[-8:]))
            self.sock.settimeout(min(remaining, 0.2))
            try:
                data, _ = self.sock.recvfrom(65535)
            except socket.timeout:
                continue
            msg = parse_msg(data)
            if msg is None:
                continue
            self.log.append("RX " + self._desc(msg))
            if pred(msg):
                return msg
            self._buf.append(data)

    @staticmethod
    def _resp_branch(msg: Dict) -> Optional[str]:
        vias = [v for n, v in msg["headers"] if n.lower() == "via"]
        return via_branch(vias[0]) if vias else None

    # ---------------- REGISTER ----------------

    def register(self, expires: int = 300, expect: str = "200",
                 timeout: float = 5.0) -> Dict:
        to = "<sip:%s@%s>" % (self.user, self.pbx[0])
        headers = [
            ("From", to + ";tag=" + self.from_tag),
            ("To", to),
            ("Call-ID", "%s@127.0.0.1" % secrets.token_hex(8)),
            ("Contact", "<sip:%s@127.0.0.1:%d>" % (self.user, self.port)),
            ("Expires", str(expires)),
            ("Max-Forwards", "70"),
        ]
        uri = "sip:%s@%s:%d" % (self.user, self.pbx[0], self.pbx[1])
        branch = self.send_request("REGISTER", uri, headers)
        resp = self.recv_matching(
            lambda m: m["kind"] == "resp" and self._resp_branch(m) == branch,
            timeout, "REGISTER response")
        if resp["status"] == 401:
            params = parse_digest(hget(resp, "www-authenticate"))
            if not params:
                raise AssertionError("401 without WWW-Authenticate Digest")
            auth = ('Digest username="%s", realm="%s", nonce="%s", '
                    'uri="%s", response="%s", algorithm=MD5, qop="auth", '
                    'nc=00000001, cnonce="0a4f113b"' % (
                        self.user, params.get("realm", self.realm),
                        params.get("nonce", ""), uri,
                        digest_response(
                            self.user, params.get("realm", self.realm),
                            self.password, "REGISTER", uri,
                            params.get("nonce", ""))))
            headers2 = headers + [("Authorization", auth)]
            # new branch/CSeq for the authenticated attempt
            branch = self.send_request("REGISTER", uri, headers2)
            resp = self.recv_matching(
                lambda m: m["kind"] == "resp"
                and self._resp_branch(m) == branch,
                timeout, "authenticated REGISTER response")
        want = expect.split(",") if expect else []
        if want and str(resp["status"]) not in want:
            raise AssertionError("REGISTER expected %s got %d %s"
                                 % (expect, resp["status"], resp["reason"]))
        return resp

    # ---------------- INVITE (caller side) ----------------

    def invite(self, target_user: str, local_rtp_port: int = 4000,
               sdp_extra: str = "a=sendrecv", timeout: float = 5.0
               ) -> Tuple[Dict, str]:
        """Send an INVITE with an SDP offer; return (100/180 resp, call-id)."""
        call_id = "%s@127.0.0.1" % secrets.token_hex(8)
        to = "<sip:%s@%s>" % (target_user, self.pbx[0])
        headers = [
            ("From", "<sip:%s@%s>;tag=%s" % (self.user, self.pbx[0],
                                             self.from_tag)),
            ("To", to),
            ("Call-ID", call_id),
            ("Contact", "<sip:%s@127.0.0.1:%d>" % (self.user, self.port)),
            ("Max-Forwards", "70"),
        ]
        sdp = self.sdp_offer(local_rtp_port, sdp_extra)
        uri = "sip:%s@%s:%d" % (target_user, self.pbx[0], self.pbx[1])
        branch = self.send_request("INVITE", uri, headers, sdp.encode())
        resp = self.recv_matching(
            lambda m: m["kind"] == "resp" and m["status"] in (100, 180, 183),
            timeout, "1xx for INVITE")
        self.dialogs[call_id] = {
            "call_id": call_id, "branch": branch, "to": to,
            "target_user": target_user, "cseq": self.cseq,
            "cseq_num": self.cseq,   # CANCEL must reuse the INVITE's CSeq number
            "rtp_port": local_rtp_port,
        }
        return resp, call_id

    def wait_answer(self, call_id: str, timeout: float = 5.0) -> Dict:
        d = self.dialogs[call_id]
        resp = self.recv_matching(
            lambda m: m["kind"] == "resp" and m["status"] in (180, 183, 200)
            and hget(m, "call-id") == call_id,
            timeout, "180/200 for INVITE")
        if resp["status"] != 200:
            resp = self.recv_matching(
                lambda m: m["kind"] == "resp" and m["status"] == 200
                and hget(m, "call-id") == call_id,
                timeout, "200 for INVITE")
        d["to_tagged"] = hget(resp, "to") or ""  # callee's To (with tag)
        # remote target = Contact of the 200 (fall back to the callee's AOR)
        contact = hget(resp, "contact") or ""
        if "<" in contact and ">" in contact:
            d["target_uri"] = contact[contact.index("<") + 1:contact.index(">")]
        else:
            d["target_uri"] = "sip:%s@127.0.0.1:%d" % (d["target_user"],
                                                       self.pbx[1])
        return resp

    def ack(self, call_id: str, request_uri: Optional[str] = None):
        d = self.dialogs[call_id]
        headers = [
            ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=z9hG4bK%s;rport"
             % (self.port, secrets.token_hex(8))),
            ("From", "<sip:%s@%s>;tag=%s" % (self.user, self.pbx[0],
                                             self.from_tag)),
            ("To", d["to_tagged"]),
            ("Call-ID", call_id),
            ("Max-Forwards", "70"),
        ]
        # ACK for 2xx is a new transaction with its own CSeq number
        self.cseq += 1
        headers.append(("CSeq", "%d ACK" % self.cseq))
        uri = request_uri or d.get(
            "target_uri", "sip:%s@127.0.0.1:%d" % (d["target_user"],
                                                   self.pbx[1]))
        data = build_msg("ACK %s SIP/2.0" % uri, headers)
        self._send(data)
        self.log.append("TX ACK " + uri)

    def bye(self, call_id: str, timeout: float = 5.0) -> Dict:
        d = self.dialogs[call_id]
        headers = [
            ("From", "<sip:%s@%s>;tag=%s" % (self.user, self.pbx[0],
                                             self.from_tag)),
            ("To", d["to_tagged"]),
            ("Call-ID", call_id),
            ("Max-Forwards", "70"),
        ]
        uri = d.get("target_uri",
                    "sip:%s@127.0.0.1:%d" % (d["target_user"], self.pbx[1]))
        branch = self.send_request("BYE", uri, headers)
        return self.recv_matching(
            lambda m: m["kind"] == "resp" and m["status"] == 200
            and hget(m, "call-id") == call_id
            and "BYE" in (hget(m, "cseq") or ""),
            timeout, "200 for BYE")

    def cancel(self, call_id: str, timeout: float = 5.0) -> Dict:
        d = self.dialogs[call_id]
        uri = "sip:%s@127.0.0.1:%d" % (d["target_user"], self.pbx[1])
        # RFC 3261 §9.1: CANCEL reuses the INVITE's branch AND its CSeq number
        # (same client transaction), only the method changes.
        data = build_msg("CANCEL %s SIP/2.0" % uri, [
            ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=%s;rport"
             % (self.port, d["branch"])),
            ("From", "<sip:%s@%s>;tag=%s" % (self.user, self.pbx[0],
                                             self.from_tag)),
            ("To", d["to"]),
            ("Call-ID", call_id),
            ("Max-Forwards", "70"),
            ("CSeq", "%d CANCEL" % d["cseq_num"]),
        ])
        self._send(data)
        self.log.append("TX CANCEL " + uri)
        return self.recv_matching(
            lambda m: m["kind"] == "resp" and m["status"] in (200, 487)
            and hget(m, "call-id") == call_id
            and "CANCEL" in (hget(m, "cseq") or ""),
            timeout, "response for CANCEL")

    # ---------------- callee side ----------------

    def wait_invite(self, timeout: float = 5.0) -> Dict:
        msg = self.recv_matching(lambda m: m["kind"] == "req"
                                 and m["method"] == "INVITE",
                                 timeout, "INVITE")
        call_id = hget(msg, "call-id")
        self.dialogs[call_id] = {
            "call_id": call_id,
            "from": hget(msg, "from"),
            "to": hget(msg, "to"),
            "src_branch": via_branch(
                [v for n, v in msg["headers"] if n.lower() == "via"][0]),
            "src_cseq": (hget(msg, "cseq") or "").split()[0],
        }
        return msg

    def trying(self, invite: Dict):
        """Relay-free 100 Trying from the callee side (optional)."""
        self._respond(invite, 100, "Trying")

    def ring(self, invite: Dict):
        self._respond(invite, 180, "Ringing")

    def answer(self, invite: Dict, local_rtp_port: int = 4002,
               sdp_extra: str = "a=sendrecv") -> str:
        """200 OK with an SDP answer; returns the To tag we generated."""
        to = hget(invite, "to") or ""
        if "tag=" not in to:
            to += ";tag=" + secrets.token_hex(4)
        sdp = self.sdp_answer(local_rtp_port, sdp_extra)
        self._respond(invite, 200, "OK", to=to, body=sdp.encode(),
                      extra=[("Contact", "<sip:%s@127.0.0.1:%d>"
                              % (self.user, self.port))])
        return to

    def reject(self, invite: Dict, code: int = 486, reason: str = "Busy Here"):
        self._respond(invite, code, reason)

    def _respond(self, invite: Dict, status: int, reason: str,
                 to: Optional[str] = None, body: bytes = b"",
                 extra: List[tuple] = ()):
        # A response MUST copy Via, From, Call-ID and CSeq from the request
        # (the PBX matches transactions on branch + CSeq method).
        headers = [(n, v) for n, v in invite["headers"]
                   if n.lower() in ("via", "from", "call-id", "cseq")]
        headers.append(("To", to or (hget(invite, "to") or "")))
        headers += list(extra)
        data = build_msg("SIP/2.0 %d %s" % (status, reason), headers, body)
        # responses go to the source of the request (the PBX on 5060)
        self._send(data)
        self.log.append("TX %d %s" % (status, reason))

    def wait_ack(self, call_id: str, timeout: float = 5.0) -> Dict:
        return self.recv_matching(
            lambda m: m["kind"] == "req" and m["method"] == "ACK"
            and hget(m, "call-id") == call_id, timeout, "ACK")

    def wait_bye(self, timeout: float = 5.0) -> Dict:
        msg = self.recv_matching(
            lambda m: m["kind"] == "req" and m["method"] == "BYE",
            timeout, "BYE")
        self._respond(msg, 200, "OK")
        self.log.append("TX 200 (for BYE)")
        return msg

    # ---------------- SDP ----------------

    @staticmethod
    def sdp_offer(rtp_port: int, extra: str = "a=sendrecv") -> str:
        return (
            "v=0\r\n"
            "o=- 0 0 IN IP4 127.0.0.1\r\n"
            "s=sip-lab call\r\n"
            "c=IN IP4 127.0.0.1\r\n"
            "t=0 0\r\n"
            "m=audio %d RTP/AVP 0 8\r\n"
            "a=rtpmap:0 PCMU/8000\r\n"
            "a=rtpmap:8 PCMA/8000\r\n"
            "%s\r\n" % (rtp_port, extra)
        )

    sdp_answer = sdp_offer  # same shape for this lab (answer uses its own port)

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass

    def __repr__(self):
        return "<UA %s port=%d>" % (self.label, self.port)
