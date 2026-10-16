#!/usr/bin/env python3
"""Attempt-A/Attempt-B process replacement oracle.

This validates durable recovery semantics with genuinely separate processes.
Product-specific runners may invoke this harness around their real Write path;
the default fixture uses a filesystem write to keep the oracle deterministic.
"""
from __future__ import annotations
import json, os, subprocess, sys, tempfile
from pathlib import Path

def atomic(path: Path, obj: dict) -> None:
    tmp=path.with_suffix(".tmp")
    tmp.write_text(json.dumps(obj,sort_keys=True))
    os.replace(tmp,path)

def read(path: Path) -> dict:
    return json.loads(path.read_text())

def append(path: Path,obj:dict)->None:
    with path.open("a") as f:
        f.write(json.dumps(obj,sort_keys=True)+"\n"); f.flush(); os.fsync(f.fileno())

def attempt_a(root:Path,effect_id:str,content:str)->None:
    receipt=root/"effect.json"; target=root/"target.txt"; log=root/"attempts.jsonl"
    atomic(receipt,{"effect_id":effect_id,"state":"INTENT_RECORDED","expected":content,"attempt":"A"})
    append(log,{"attempt":"A","state":"INTENT_RECORDED","effect_id":effect_id})
    atomic(receipt,{"effect_id":effect_id,"state":"DISPATCHED","expected":content,"attempt":"A"})
    append(log,{"attempt":"A","state":"DISPATCHED","effect_id":effect_id})
    target.write_text(content)
    with target.open("r+") as f: f.flush(); os.fsync(f.fileno())
    append(log,{"attempt":"A","state":"DOWNSTREAM_COMMITTED","effect_id":effect_id})
    os._exit(86)

def reconcile(root:Path,effect_id:str)->str:
    receipt=read(root/"effect.json")
    if receipt.get("effect_id")!=effect_id: return "IDENTITY_MISMATCH"
    target=root/"target.txt"; expected=receipt["expected"]
    try: actual=target.read_text()
    except FileNotFoundError: return "RETRY_ALLOWED"
    except OSError: return "STILL_UNCERTAIN"
    if actual==expected: return "RECONCILED_SUCCESS"
    return "STILL_UNCERTAIN"

def attempt_b(root:Path,effect_id:str)->str:
    decision=reconcile(root,effect_id)
    append(root/"attempts.jsonl",{"attempt":"B","decision":decision,"effect_id":effect_id})
    if decision=="RECONCILED_SUCCESS":
        r=read(root/"effect.json"); atomic(root/"effect.json",{**r,"state":"RECONCILED_SUCCESS","attempt":"B"})
        return decision
    if decision=="RETRY_ALLOWED":
        r=read(root/"effect.json")
        (root/"target.txt").write_text(r["expected"])
        atomic(root/"effect.json",{**r,"state":"CONFIRMED_SUCCESS","attempt":"B"})
        append(root/"attempts.jsonl",{"attempt":"B","state":"REDISPATCHED","effect_id":effect_id})
        return decision
    return decision

def run_child(root:Path,effect_id:str,content:str)->int:
    p=subprocess.run([sys.executable,__file__,"--a",str(root),effect_id,content])
    assert p.returncode==86,p.returncode
    return p.returncode

def main()->None:
    if len(sys.argv)>1 and sys.argv[1]=="--a":
        attempt_a(Path(sys.argv[2]),sys.argv[3],sys.argv[4]); return
    results=[]
    with tempfile.TemporaryDirectory(prefix="attempt-replacement-") as td:
        root=Path(td); eid="effect-001"; run_child(root,eid,"expected")
        assert attempt_b(root,eid)=="RECONCILED_SUCCESS"
        assert (root/"target.txt").read_text()=="expected"
        lines=[json.loads(x) for x in (root/"attempts.jsonl").read_text().splitlines()]
        assert sum(x.get("state")=="DOWNSTREAM_COMMITTED" for x in lines)==1
        assert not any(x.get("state")=="REDISPATCHED" for x in lines)
        assert attempt_b(root,eid)=="RECONCILED_SUCCESS"
        assert reconcile(root,"other-effect")=="IDENTITY_MISMATCH"
        results.append("committed_then_reconciled_no_redispatch")
    with tempfile.TemporaryDirectory(prefix="attempt-replacement-") as td:
        root=Path(td); eid="effect-002"; run_child(root,eid,"expected"); (root/"target.txt").unlink()
        assert attempt_b(root,eid)=="RETRY_ALLOWED"
        assert (root/"target.txt").read_text()=="expected"
        results.append("known_absence_allows_single_retry")
    with tempfile.TemporaryDirectory(prefix="attempt-replacement-") as td:
        root=Path(td); eid="effect-003"; run_child(root,eid,"expected"); (root/"target.txt").write_text("conflict")
        assert attempt_b(root,eid)=="STILL_UNCERTAIN"
        assert (root/"target.txt").read_text()=="conflict"
        results.append("conflict_fails_closed")
    with tempfile.TemporaryDirectory(prefix="attempt-replacement-") as td:
        root=Path(td); (root/"effect.json").write_text("{broken")
        try: attempt_b(root,"effect-004")
        except json.JSONDecodeError: results.append("corrupt_receipt_fails_closed")
        else: raise AssertionError("corrupt receipt accepted")
    print(json.dumps({"schema":"attempt-replacement-oracle/v1","separate_processes":True,"assertions":"passed","cases":results},indent=2))

if __name__=="__main__": main()
