#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Shared SIP helpers for the sip-lab: message parse/build, URI helpers,
digest auth. Standard library only.

Written for a deterministic local lab: UDP, IPv4, no compact-header
special-casing beyond what pjsua/MicroSIP actually emit.
"""
from __future__ import annotations

import hashlib
import hmac
import re
from typing import Dict, List, Optional, Tuple

# msg = {"kind": "req"|"resp", "start": str, "headers": [(name, value)],
#        "body": bytes, plus "method"/"uri" or "status"/"reason"}


def md5_hex(s: str) -> str:
    return hashlib.md5(s.encode("utf-8")).hexdigest()


# --------------------------------------------------------------------------
# parsing
# --------------------------------------------------------------------------

def parse_msg(data: bytes) -> Optional[Dict]:
    """Parse a raw datagram into a message dict. Returns None if malformed."""
    if not data:
        return None
    if b"\r\n\r\n" in data:
        head, body = data.split(b"\r\n\r\n", 1)
    elif b"\n\n" in data:
        head, body = data.split(b"\n\n", 1)
    else:
        head, body = data, b""
    text = head.decode("utf-8", "replace")
    lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    if not lines or not lines[0].strip():
        return None
    start = lines[0].strip()
    headers: List[Tuple[str, str]] = []
    for line in lines[1:]:
        if not line:
            continue
        if line[0] in " \t" and headers:  # header folding
            n, v = headers[-1]
            headers[-1] = (n, v + " " + line.strip())
        elif ":" in line:
            n, v = line.split(":", 1)
            headers.append((n.strip(), v.strip()))
        # else: garbage line -> ignore
    msg: Dict = {"start": start, "headers": headers, "body": body}
    if start.startswith("SIP/"):
        parts = start.split(" ", 2)
        if len(parts) < 2 or not parts[1].isdigit():
            return None
        msg["kind"] = "resp"
        msg["status"] = int(parts[1])
        msg["reason"] = parts[2] if len(parts) > 2 else ""
    else:
        parts = start.split(" ")
        if len(parts) < 3 or not parts[0]:
            return None
        msg["kind"] = "req"
        msg["method"] = parts[0]
        msg["uri"] = parts[1]
    return msg


def hget(msg: Dict, name: str) -> Optional[str]:
    lname = name.lower()
    for n, v in msg["headers"]:
        if n.lower() == lname:
            return v
    return None


def hget_all(msg: Dict, name: str) -> List[str]:
    lname = name.lower()
    return [v for n, v in msg["headers"] if n.lower() == lname]


def build_msg(start: str, headers: List[Tuple[str, str]], body: bytes = b"") -> bytes:
    """Serialize a message. Any Content-Length in `headers` is ignored and
    recomputed from `body` (we never modify bodies, so this is always right)."""
    lines = [start]
    for n, v in headers:
        if n.lower() == "content-length":
            continue
        lines.append("%s: %s" % (n, v))
    lines.append("Content-Length: %d" % len(body))
    return ("\r\n".join(lines) + "\r\n\r\n").encode("utf-8") + body


# --------------------------------------------------------------------------
# URI helpers
# --------------------------------------------------------------------------

def _uri_core(uri: str) -> str:
    u = (uri or "").strip()
    # name-addr: "Display" <sip:user@host> -> sip:user@host
    if "<" in u and ">" in u:
        u = u[u.find("<") + 1 : u.find(">")].strip()
    else:
        u = u.strip("<>")
    u = u.split("?", 1)[0]
    m = re.match(r"^(?:sips?):", u, re.I)
    return u[m.end():] if m else u


def uri_user(uri: str) -> Optional[str]:
    """user part of a SIP URI, or None (no scheme / no @ / no user)."""
    core = _uri_core(uri)
    if "@" not in core:
        return None
    user = core.rsplit("@", 1)[0]
    user = user.split(";", 1)[0]
    return user or None


def uri_host(uri: str) -> Optional[str]:
    """host (no port, no params) of a SIP URI, or None."""
    core = _uri_core(uri)
    if "@" in core:
        hp = core.rsplit("@", 1)[1]
    else:
        hp = core
    hp = hp.split(";", 1)[0].strip()
    if not hp:
        return None
    if hp.startswith("["):
        m = re.match(r"^\[([^\]]+)\]", hp)
        return m.group(1) if m else None
    return hp.rsplit(":", 1)[0] if ":" in hp else hp


def uri_dest(uri: str, default_port: Optional[int] = 5060) -> Tuple[str, Optional[int]]:
    """(host, port) of a SIP URI; port is None when absent and default_port is None."""
    core = _uri_core(uri)
    if "@" in core:
        hp = core.rsplit("@", 1)[1]
    else:
        hp = core
    hp = hp.split(";", 1)[0].strip()
    if not hp:
        return "", default_port
    if hp.startswith("["):  # IPv6 literal (parsed, but the lab is IPv4-only)
        m = re.match(r"^\[([^\]]+)\](?::(\d+))?$", hp)
        if m:
            return m.group(1), int(m.group(2)) if m.group(2) else default_port
        return hp, default_port
    if ":" in hp:
        h, p = hp.rsplit(":", 1)
        if p.isdigit():
            return h, int(p)
    return hp, default_port


def via_branch(v: Optional[str]) -> Optional[str]:
    m = re.search(r"(?:^|;)\s*branch=([^;\s]+)", v or "")
    return m.group(1) if m else None


# --------------------------------------------------------------------------
# digest auth (MD5, qop=auth) — enough for pjsua / MicroSIP
# --------------------------------------------------------------------------

def parse_digest(hval: Optional[str]) -> Optional[Dict[str, str]]:
    if not hval:
        return None
    hval = hval.strip()
    if not re.match(r"^Digest\s", hval, re.I):
        return None
    params: Dict[str, str] = {}
    for m in re.finditer(r'(\w+)=("([^"]*)"|([^,\s]+))', hval):
        params[m.group(1).lower()] = m.group(3) if m.group(3) is not None else m.group(4)
    return params


def digest_response(username: str, realm: str, password: str, method: str,
                    uri: str, nonce: str, nc: str = "00000001",
                    cnonce: str = "0a4f113b", qop: str = "auth") -> str:
    ha1 = md5_hex("%s:%s:%s" % (username, realm, password))
    ha2 = md5_hex("%s:%s" % (method, uri))
    if qop:
        return md5_hex("%s:%s:%s:%s:%s:%s" % (ha1, nonce, nc, cnonce, qop, ha2))
    return md5_hex("%s:%s:%s" % (ha1, nonce, ha2))


def check_digest(params: Dict[str, str], method: str, req_uri: str,
                 accounts: Dict[str, str], expected_realm: Optional[str] = None
                 ) -> Tuple[bool, str]:
    """Validate a Digest Authorization against `accounts` (user -> password).

    Returns (ok, reason) — reason is log-friendly, never contains secrets.
    """
    if not params:
        return False, "missing"
    user = params.get("username", "")
    realm = params.get("realm", "")
    nonce = params.get("nonce", "")
    if not nonce:
        return False, "no-nonce"
    if expected_realm is not None and realm != expected_realm:
        return False, "realm-mismatch(sent=%s expected=%s)" % (realm, expected_realm)
    if user not in accounts:
        return False, "unknown-user(%s)" % user
    uri = params.get("uri") or req_uri
    ha1 = md5_hex("%s:%s:%s" % (user, realm, accounts[user]))
    ha2 = md5_hex("%s:%s" % (method, uri))
    qop = params.get("qop", "")
    if qop:
        expected = md5_hex("%s:%s:%s:%s:%s:%s" % (
            ha1, nonce, params.get("nc", ""), params.get("cnonce", ""), qop, ha2))
    else:
        expected = md5_hex("%s:%s:%s" % (ha1, nonce, ha2))
    got = (params.get("response") or "").lower()
    if not hmac.compare_digest(expected, got):
        return False, "bad-response"
    return True, "ok"


def sdp_summary(body: bytes) -> List[str]:
    """Log-friendly SDP lines (c= and m=) — the evidence of the offer/answer."""
    out = []
    try:
        text = body.decode("utf-8", "replace")
    except Exception:
        return out
    for line in text.splitlines():
        if line.startswith(("c=", "m=")):
            out.append(line.strip())
    return out
