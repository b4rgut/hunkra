#!/usr/bin/env python3
"""Verify finite native path/focus/long-line/history/recent/draft scenarios from an arbitrary cwd with an independent Git byte oracle."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Run only disposable native fixtures; settings/clipboard/owner repositories remain independent."""
    with tempfile.TemporaryDirectory(prefix="hunkra-native-ux-") as temporary:
        fixture = Path(temporary)
        environment = {key:value for key,value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary,XDG_CONFIG_HOME=str(fixture/"config"),GIT_CONFIG_GLOBAL="/dev/null",GIT_CONFIG_NOSYSTEM="1",GIT_ATTR_NOSYSTEM="1")

        def git(root,*args,success=True):
            """Read/write the private fixture through independent Git, without optional index refresh locks."""
            result=subprocess.run(["git","--no-optional-locks","-c","user.name=UX Fixture","-c","user.email=fixture@example.invalid",*args],cwd=root,env=environment,capture_output=True,timeout=30)
            if success: assert result.returncode==0,result.stderr.decode(errors="replace")
            return result.stdout

        reports=[]
        for action in ("ux-paths","ux-history","ux-draft"):
            root=fixture/action;root.mkdir();git(root,"init","-b","main")
            long=("Длинная строка 🦀\t"+"0123456789"*650).encode()
            (root/"f").write_bytes(long+b"base\n");(root/"sibling").write_bytes(b"sibling\r\nEOF")
            git(root,"add",".");git(root,"commit","-m","long base")
            if action=="ux-draft":
                git(root,"switch","-c","incoming");(root/"f").write_bytes(b"incoming\n");git(root,"commit","-am","incoming");git(root,"switch","main")
                (root/"f").write_bytes(b"current\n");git(root,"commit","-am","current");git(root,"merge","--no-edit","incoming",success=False)
            else:
                (root/"f").write_bytes(long+b"staged\n");git(root,"add","f");(root/"f").write_bytes(long+b"worktree EOF")
            (root/"src").mkdir();(root/"src/Ядро.bsl").write_bytes(b"one\n");(root/"src/MoreЯдро.bsl").write_bytes(b"two\n")
            (root/"untracked").mkdir()
            for number in range(2000): (root/f"untracked/{number:04}.txt").write_bytes(b"file\n")
            recent=fixture/(action+"-recent");recent.mkdir();git(recent,"init","-b","main");(recent/"f").write_bytes(b"base\n");git(recent,"add","f");git(recent,"commit","-m","recent");(recent/"f").write_bytes(b"new\n")
            settings=fixture/"config/hunkra/settings.json";settings.parent.mkdir(parents=True,exist_ok=True);settings.write_text(json.dumps({"locale":"En","recent":[str(root),str(recent)]}))
            settings_before=settings.read_bytes()
            before={item:{"head":git(item,"rev-parse","HEAD"),"index":(item/".git/index").read_bytes(),"stages":git(item,"ls-files","--stage"),"status":git(item,"status","--porcelain=v1","-z","--untracked-files=all"),"files":{str(path.relative_to(item)):path.read_bytes() for path in item.rglob("*") if path.is_file() and ".git" not in path.relative_to(item).parts}} for item in (root,recent)}
            report=fixture/(action+".json")
            result=subprocess.run([str(Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra"))),str(root),"--smoke-test",str(report),"--smoke-operation",action],cwd=fixture,env=dict(environment,HUNKRA_UX_RECENT=str(recent)),capture_output=True,timeout=40)
            assert result.returncode==0,result.stderr.decode(errors="replace")
            evidence=json.loads(report.read_text());assert evidence["ok"] and evidence["native_display"] and evidence["read_only"]
            assert evidence["locales"]==["Changes","Изменения","Изменения"]
            for item,state in before.items():
                assert git(item,"rev-parse","HEAD")==state["head"] and (item/".git/index").read_bytes()==state["index"] and git(item,"ls-files","--stage")==state["stages"] and git(item,"status","--porcelain=v1","-z","--untracked-files=all")==state["status"]
                assert all((item/path).read_bytes()==data for path,data in state["files"].items())
            assert settings.read_bytes()==settings_before
            reports.append(evidence)
        destination.write_text(json.dumps({"ok":True,"runs":reports,"oracle":"independent Git HEAD/index/stages/status and every fixture worktree/settings byte invariant"},ensure_ascii=False,indent=2)+"\n")
        print("Native UX smoke: 3 runs passed")


if __name__=="__main__":
    target=Path(sys.argv[1]);assert target.is_absolute();smoke(target)
