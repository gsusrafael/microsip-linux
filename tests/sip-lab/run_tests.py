#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Deterministic test suite for the sip-lab PBX (protocol level).

Covers, in order:
  1.  REGISTER without credentials      -> 401 + Digest challenge
  2.  REGISTER with wrong password      -> 401 (auth fails)
  3.  REGISTER (1001, 1002, 1003)       -> 200 + Contact/Expires
  4.  OPTIONS keepalive                 -> 200
  5.  Full call 1001 -> 1002: INVITE (SDP offer) -> 100 -> 180 -> 200 (SDP
      answer) -> ACK -> BYE -> 200, with the SDP offer/answer verified at
      both ends and the Record-Route/branching verified in the messages
  6.  Busy line: second INVITE to a ringing extension -> 486
  7.  CANCEL of an early dialog        -> 200(CANCEL) + 487(INVITE)
  8.  INVITE to an unregistered ext    -> 404
  9.  Unregister (Expires: 0)          -> 200, then INVITE -> 404
 10.  Digest nonce is single-use-ish   -> stale/unknown nonce re-challenges

Run:  python3 run_tests.py            (PBX must be up on 127.0.0.1:5060)
Exit code 0 = all tests passed.
"""
from __future__ import annotations

import json
import os
import sys
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sip_client import PBX, SipTimeout, UA  # noqa: E402
from siplib import hget, uri_user  # noqa: E402

LAB = os.path.dirname(os.path.abspath(__file__))

PASS = 0
FAIL = 0
RESULTS = []


def check(name: str, cond: bool, detail: str = ""):
    global PASS, FAIL
    if cond:
        PASS += 1
        RESULTS.append(("PASS", name, detail))
        print("  PASS  %s%s" % (name, (" — " + detail) if detail else ""))
    else:
        FAIL += 1
        RESULTS.append(("FAIL", name, detail))
        print("  FAIL  %s%s" % (name, (" — " + detail) if detail else ""))


def load_accounts():
    with open(os.path.join(LAB, "accounts.json"), encoding="utf-8") as f:
        doc = json.load(f)
    return {u: v["password"] for u, v in doc["users"].items()}, doc["realm"]


def t1_to_t4(accounts, realm):
    print("\n[1] REGISTER without credentials -> 401 Digest challenge")
    ua = UA("1001", accounts["1001"], realm, label="alice")
    # raw REGISTER without auth
    from siplib import build_msg
    ua.cseq += 1
    uri = "sip:1001@%s:%d" % (ua.pbx[0], ua.pbx[1])
    data = build_msg("REGISTER %s SIP/2.0" % uri, [
        ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=z9hG4bKtest1;rport"
         % ua.port),
        ("From", "<sip:1001@127.0.0.1>;tag=t1"),
        ("To", "<sip:1001@127.0.0.1>"),
        ("Call-ID", "t1@127.0.0.1"),
        ("Contact", "<sip:1001@127.0.0.1:%d>" % ua.port),
        ("Expires", "300"),
        ("Max-Forwards", "70"),
        ("CSeq", "%d REGISTER" % ua.cseq),
    ])
    ua._send(data)
    resp = ua.recv_matching(lambda m: m["kind"] == "resp", 5, "401")
    wa = hget(resp, "www-authenticate") or ""
    check("401 without credentials", resp["status"] == 401,
          "got %d" % resp["status"])
    check("401 carries Digest challenge",
          "Digest" in wa and "nonce=" in wa and "qop=" in wa,
          wa[:80])

    print("\n[2] REGISTER with wrong password -> 401 again")
    from siplib import digest_response
    wrong = UA("1001", "definitely-wrong", realm, label="alice-wrong")
    branch = wrong.register(expires=300, expect="401")
    check("wrong password rejected", branch["status"] == 401,
          "got %d" % branch["status"])
    wrong.close()

    print("\n[3] REGISTER 1001/1002/1003 -> 200 with binding")
    alice = UA("1001", accounts["1001"], realm, label="alice")
    bob = UA("1002", accounts["1002"], realm, label="bob")
    carol = UA("1003", accounts["1003"], realm, label="carol")
    for ua, ext in ((alice, "1001"), (bob, "1002"), (carol, "1003")):
        resp = ua.register(expires=300)
        contact = hget(resp, "contact") or ""
        check("register %s -> 200" % ext, resp["status"] == 200,
              "got %d" % resp["status"])
        check("register %s echoes Contact+expires" % ext,
              str(ua.port) in contact and "expires=300" in contact,
              contact)

    print("\n[4] OPTIONS keepalive -> 200")
    from siplib import build_msg as _bm
    alice.cseq += 1
    data = _bm("OPTIONS sip:1001@127.0.0.1:5060 SIP/2.0", [
        ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=z9hG4bKtest4;rport"
         % alice.port),
        ("From", "<sip:1001@127.0.0.1>;tag=t4"),
        ("To", "<sip:1001@127.0.0.1>"),
        ("Call-ID", "t4@127.0.0.1"),
        ("Max-Forwards", "70"),
        ("CSeq", "%d OPTIONS" % alice.cseq),
    ])
    alice._send(data)
    resp = alice.recv_matching(lambda m: m["kind"] == "resp", 5, "200")
    check("OPTIONS -> 200", resp["status"] == 200, "got %d" % resp["status"])
    check("OPTIONS advertises Allow", "INVITE" in (hget(resp, "allow") or ""))

    return alice, bob, carol


def t5_full_call(alice, bob):
    print("\n[5] Full call 1001 -> 1002 (INVITE/180/200 + SDP exchange)")
    invite_seen = {}

    # Bob waits for the INVITE in the background of the flow: we drive both
    # UAs from this single thread by interleaving waits.
    # Step A: alice sends INVITE, PBX replies 100 Trying.
    # We need bob to receive the forwarded INVITE before alice can get 180,
    # so: alice.invite() returns on the 1xx; but bob must be polled first.
    # Simplest deterministic order: start alice's INVITE, then poll bob.
    import threading
    result = {}

    def alice_side():
        try:
            resp, call_id = alice.invite("1002", local_rtp_port=4000)
            result["early"] = resp
            result["call_id"] = call_id
            final = alice.wait_answer(call_id, timeout=8)
            result["final"] = final
            alice.ack(call_id)
            result["ack_sent"] = True
            bye_resp = alice.bye(call_id, timeout=8)
            result["bye_resp"] = bye_resp
        except Exception as e:  # noqa: BLE001
            result["error"] = traceback.format_exc() or repr(e)

    th = threading.Thread(target=alice_side, daemon=True)
    th.start()

    invite = bob.wait_invite(timeout=8)
    invite_seen["msg"] = invite
    check("INVITE reaches 1002", True,
          "from %s" % (uri_user(hget(invite, "from") or "") or "?"))
    body = invite.get("body") or b""
    check("INVITE carries SDP offer",
          b"m=audio 4000 RTP/AVP" in body and b"c=IN IP4 127.0.0.1" in body,
          body.decode("utf-8", "replace").splitlines()[3] if body else "empty")
    check("INVITE has Record-Route (PBX stays in path)",
          "lr" in (hget(invite, "record-route") or ""),
          hget(invite, "record-route") or "none")

    bob.ring(invite)
    to_tag = bob.answer(invite, local_rtp_port=4002)
    check("180 + 200 sent by 1002", True, "to-tag generated: %s" % to_tag[:12])

    ack = bob.wait_ack(result.get("call_id", ""), timeout=8)
    check("ACK arrives at 1002", ack["method"] == "ACK",
          "CSeq %s" % (hget(ack, "cseq") or "?"))
    ack_body = ack.get("body") or b""
    check("ACK has no SDP (answer already exchanged)", not ack_body,
          "%d bytes" % len(ack_body))

    bye = bob.wait_bye(timeout=8)
    check("BYE arrives at 1002", bye["method"] == "BYE")

    th.join(timeout=10)
    if "error" in result:
        check("caller side flow completed", False, result["error"])
        return
    early = result.get("early")
    check("caller saw 1xx", early is not None and early["status"] in (100, 180, 183),
          "%d %s" % (early["status"], early["reason"]) if early else "none")
    final = result.get("final")
    check("caller got 200 OK", final is not None and final["status"] == 200,
          "%d" % final["status"] if final else "none")
    if final is not None:
        fbody = final.get("body") or b""
        check("200 carries SDP answer (1002's port 4002)",
              b"m=audio 4002 RTP/AVP" in fbody,
              next((l for l in fbody.decode("utf-8", "replace").splitlines()
                    if l.startswith("m=")), "no m="))
        check("200 has callee To-tag", "tag=" in (hget(final, "to") or ""))
    check("caller sent ACK", result.get("ack_sent") is True)
    bye_resp = result.get("bye_resp")
    check("caller got 200 for BYE", bye_resp is not None
          and bye_resp["status"] == 200,
          "%d" % bye_resp["status"] if bye_resp else "none")


def t6_busy(alice, carol):
    print("\n[6] Second INVITE to a ringing line -> 486 Busy Here")
    import threading
    result = {}

    def carol_side():
        try:
            invite = carol.wait_invite(timeout=8)
            result["invite"] = invite
            carol.ring(invite)  # keep it ringing (no answer yet)
            # hold the early dialog until the test finishes
            import time
            time.sleep(3)
        except Exception:  # noqa: BLE001
            result["error"] = traceback.format_exc()

    th = threading.Thread(target=carol_side, daemon=True)
    th.start()
    resp, call_id = alice.invite("1003", local_rtp_port=4100, timeout=8)
    # first call is now early (ringing); second call to same ext:
    from siplib import build_msg
    alice.cseq += 1
    call_id2 = "busy2@127.0.0.1"
    data = build_msg("INVITE sip:1003@127.0.0.1:5060 SIP/2.0", [
        ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=z9hG4bKbusy2;rport"
         % alice.port),
        ("From", "<sip:1001@127.0.0.1>;tag=%s" % alice.from_tag),
        ("To", "<sip:1003@127.0.0.1>"),
        ("Call-ID", call_id2),
        ("Contact", "<sip:1001@127.0.0.1:%d>" % alice.port),
        ("Max-Forwards", "70"),
        ("CSeq", "%d INVITE" % alice.cseq),
        ("Content-Type", "application/sdp"),
    ], alice.sdp_offer(4102).encode())
    alice._send(data)
    resp2 = alice.recv_matching(
        lambda m: m["kind"] == "resp" and m["status"] in (100, 180, 486, 487)
        and hget(m, "call-id") == call_id2, 8, "486")
    check("second INVITE rejected with 486", resp2["status"] == 486,
          "got %d %s" % (resp2["status"], resp2["reason"]))
    check("486 carries a To-tag", "tag=" in (hget(resp2, "to") or ""))
    th.join(timeout=6)


def t7_cancel(alice, bob):
    print("\n[7] CANCEL of an early dialog -> 200(CANCEL) + 487(INVITE)")
    import threading
    result = {}

    def bob_side():
        try:
            invite = bob.wait_invite(timeout=8)
            result["invite"] = invite
            bob.ring(invite)
            # wait for either ACK (answered) or CANCEL (terminated)
            msg = bob.recv_matching(
                lambda m: m["kind"] == "req" and m["method"] in ("ACK", "CANCEL"),
                8, "CANCEL or ACK")
            result["got"] = msg["method"]
            if msg["method"] == "CANCEL":
                bob._respond(msg, 200, "OK")          # 200 to CANCEL
                bob._respond(invite, 487, "Request Terminated")  # 487 to INVITE
        except Exception:  # noqa: BLE001
            result["error"] = traceback.format_exc()

    th = threading.Thread(target=bob_side, daemon=True)
    th.start()
    resp, call_id = alice.invite("1002", local_rtp_port=4200, timeout=8)
    check("call is early (no answer yet)", resp["status"] in (100, 180, 183),
          "%d" % resp["status"])
    cancel_resp = alice.cancel(call_id, timeout=8)
    check("CANCEL answered", cancel_resp["status"] in (200, 487),
          "%d %s (CSeq %s)" % (cancel_resp["status"], cancel_resp["reason"],
                               hget(cancel_resp, "cseq") or "?"))
    # after CANCEL, the INVITE transaction ends with 487
    final = alice.recv_matching(
        lambda m: m["kind"] == "resp" and m["status"] == 487
        and hget(m, "call-id") == call_id, 8, "487 for INVITE")
    check("INVITE terminated with 487", final["status"] == 487,
          "got %d" % final["status"])
    th.join(timeout=6)
    check("callee saw CANCEL", result.get("got") == "CANCEL",
          result.get("got", "none"))


def t8_t10(alice, bob):
    print("\n[8] INVITE to an unregistered extension -> 404")
    from siplib import build_msg
    alice.cseq += 1
    call_id = "nf@127.0.0.1"
    data = build_msg("INVITE sip:1999@127.0.0.1:5060 SIP/2.0", [
        ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=z9hG4bKnf;rport" % alice.port),
        ("From", "<sip:1001@127.0.0.1>;tag=nf1"),
        ("To", "<sip:1999@127.0.0.1>"),
        ("Call-ID", call_id),
        ("Contact", "<sip:1001@127.0.0.1:%d>" % alice.port),
        ("Max-Forwards", "70"),
        ("CSeq", "%d INVITE" % alice.cseq),
        ("Content-Type", "application/sdp"),
    ], alice.sdp_offer(4300).encode())
    alice._send(data)
    resp = alice.recv_matching(
        lambda m: m["kind"] == "resp" and m["status"] in (404, 486)
        and hget(m, "call-id") == call_id, 8, "404")
    check("unknown ext -> 404", resp["status"] == 404,
          "got %d %s" % (resp["status"], resp["reason"]))
    # (the ACK for this non-2xx response goes to the PBX itself — it generated
    #  the response — so it never reaches us; nothing to consume here)

    print("\n[9] Unregister (Expires: 0) -> 200, then INVITE -> 404")
    resp = bob.register(expires=0)
    check("unregister -> 200", resp["status"] == 200, "got %d" % resp["status"])
    check("unregister echoes Expires: 0",
          (hget(resp, "expires") or "").strip() == "0",
          "Expires=%s" % (hget(resp, "expires") or "?"))
    alice.cseq += 1
    call_id = "gone@127.0.0.1"
    data = build_msg("INVITE sip:1002@127.0.0.1:5060 SIP/2.0", [
        ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=z9hG4bKgone;rport"
         % alice.port),
        ("From", "<sip:1001@127.0.0.1>;tag=g1"),
        ("To", "<sip:1002@127.0.0.1>"),
        ("Call-ID", call_id),
        ("Contact", "<sip:1001@127.0.0.1:%d>" % alice.port),
        ("Max-Forwards", "70"),
        ("CSeq", "%d INVITE" % alice.cseq),
        ("Content-Type", "application/sdp"),
    ], alice.sdp_offer(4400).encode())
    alice._send(data)
    resp = alice.recv_matching(
        lambda m: m["kind"] == "resp" and m["status"] in (404, 486)
        and hget(m, "call-id") == call_id, 8, "404 after unreg")
    check("INVITE to unregistered 1002 -> 404", resp["status"] == 404,
          "got %d" % resp["status"])

    print("\n[10] Stale/unknown digest nonce -> fresh 401 challenge")
    from siplib import build_msg as _bm, digest_response
    ua = UA("1001", alice.password, alice.realm, label="stale")
    ua.cseq += 1
    uri = "sip:1001@127.0.0.1:5060"
    auth = ('Digest username="1001", realm="%s", nonce="deadbeefdeadbeef", '
            'uri="%s", response="%s", algorithm=MD5, qop="auth", '
            'nc=00000001, cnonce="0a4f113b"' % (
                alice.realm, uri,
                digest_response("1001", alice.realm, alice.password,
                                "REGISTER", uri, "deadbeefdeadbeef")))
    data = _bm("REGISTER %s SIP/2.0" % uri, [
        ("Via", "SIP/2.0/UDP 127.0.0.1:%d;branch=z9hG4bKstale;rport"
         % ua.port),
        ("From", "<sip:1001@127.0.0.1>;tag=st"),
        ("To", "<sip:1001@127.0.0.1>"),
        ("Call-ID", "stale@127.0.0.1"),
        ("Contact", "<sip:1001@127.0.0.1:%d>" % ua.port),
        ("Expires", "300"),
        ("Max-Forwards", "70"),
        ("Authorization", auth),
        ("CSeq", "%d REGISTER" % ua.cseq),
    ])
    ua._send(data)
    resp = ua.recv_matching(lambda m: m["kind"] == "resp", 5, "401 stale")
    check("unknown nonce re-challenged with 401", resp["status"] == 401,
          "got %d" % resp["status"])
    check("fresh nonce issued", "nonce=" in (hget(resp, "www-authenticate") or ""))
    ua.close()


def main() -> int:
    accounts, realm = load_accounts()
    print("sip-lab test suite — PBX %s:%d realm=%s exts=%s"
          % (PBX[0], PBX[1], realm, ",".join(sorted(accounts))))
    try:
        alice, bob, carol = t1_to_t4(accounts, realm)
    except SipTimeout as e:
        print("FATAL: PBX not reachable or too slow: %s" % e)
        return 2
    try:
        t5_full_call(alice, bob)
        t6_busy(alice, carol)
        t7_cancel(alice, bob)
        t8_t10(alice, bob)
    except SipTimeout as e:
        check("suite ran to completion", False, "SipTimeout: %s" % e)
    except Exception:  # noqa: BLE001
        check("suite ran to completion", False, traceback.format_exc())
    finally:
        for ua in (alice, bob, carol):
            ua.close()

    print("\n==== %d passed, %d failed ====" % (PASS, FAIL))
    out = os.path.join(LAB, "evidencia", "test-results.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        json.dump({"pass": PASS, "fail": FAIL,
                   "results": [{"status": s, "name": n, "detail": d}
                               for s, n, d in RESULTS]}, f, indent=2)
    print("results -> %s" % out)
    return 0 if FAIL == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
