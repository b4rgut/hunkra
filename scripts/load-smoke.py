#!/usr/bin/env python3
"""Verify finite real native load/event/virtualization windows with an independent Git oracle in private repositories."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def smoke(destination):
    """Preserve HEAD/index/staged/large/other bytes across native automatic updates, storms and degraded periodic refresh."""
    with tempfile.TemporaryDirectory(prefix="hunkra-native-load-") as temporary:
        fixture=Path(temporary)
        environment={key:value for key,value in os.environ.items() if not key.startswith("GIT_")}
        environment.update(HOME=temporary,XDG_CONFIG_HOME=str(fixture/"config"),GIT_CONFIG_GLOBAL="/dev/null",GIT_CONFIG_NOSYSTEM="1",GIT_ATTR_NOSYSTEM="1")
        reports=[]

        def git(root,*args,success=True):
            """Use only the fixture's independent Git; readonly queries cannot refresh its index through optional locks."""
            result=subprocess.run(["git","--no-optional-locks","-c","user.name=Native Load Fixture","-c","user.email=fixture@example.invalid",*args],cwd=root,env=environment,capture_output=True,timeout=30)
            if success:
                assert result.returncode==0,result.stderr.decode(errors="replace")
            return result.stdout

        for action in ("load-watch","load-mass","load-large","load-periodic","load-draft"):
            root=fixture/action; root.mkdir(); git(root,"init","-b","main")
            content=b"\xef\xbb\xbffixture\r\nno EOF newline"
            files=root/"files";files.mkdir()
            for number in range(3000): (files/f"{number:04}.txt").write_bytes(content)
            (root/"f").write_bytes(b"base\n");(root/"sibling").write_bytes(b"sibling\r\nEOF")
            large=b"".join(f"line {number:05} stable\n".encode() for number in range(18000))
            dense=b"\n"*(2*1024*1024)
            if action=="load-large": (root/"dense.txt").write_bytes(dense)
            (root/"large.txt").write_bytes(large); git(root,"add",".");git(root,"commit","-m","base")
            if action=="load-draft":
                git(root,"switch","-c","incoming");(root/"f").write_bytes(b"incoming\n");git(root,"commit","-am","incoming");git(root,"switch","main")
                (root/"f").write_bytes(b"current\n");git(root,"commit","-am","current");git(root,"merge","--no-edit","incoming",success=False)
            else:
                (root/"f").write_bytes(b"staged\n");git(root,"add","f");(root/"f").write_bytes(b"after\n")
            (root/"large.txt").write_bytes(large.replace(b"line 09000 stable",b"line 09000 change"))
            (root/"oversized").write_bytes(b"x"*(2*1024*1024+1))
            if action=="load-large": (root/"dense.txt").write_bytes(b"dense worktree\r\nEOF")
            untracked=root/"untracked";untracked.mkdir()
            for number in range(4000): (untracked/f"{number:04}.txt").write_bytes(b"untracked\n")
            count=len([item for item in git(root,"status","--porcelain=v1","-z","--untracked-files=all").split(b"\0") if item])
            index=(root/".git/index").read_bytes();head=git(root,"rev-parse","HEAD");stages=git(root,"ls-files","--stage");f=(root/"f").read_bytes()
            report_path=fixture/(action+".json")
            env=dict(environment,HUNKRA_SMOKE_EXPECT_FILES=str(count))
            result=subprocess.run([str(Path(os.environ.get("HUNKRA_TEST_BINARY", ROOT / "target/debug/hunkra"))),str(root),"--smoke-test",str(report_path),"--smoke-operation",action],env=env,capture_output=True,timeout=40)
            assert result.returncode==0,result.stderr.decode(errors="replace")
            report=json.loads(report_path.read_text());assert report["ok"] and report["native_display"] and report["message_retained"]
            assert report["locales"]==["Changes","Изменения","Изменения"]
            assert (root/".git/index").read_bytes()==index and git(root,"rev-parse","HEAD")==head and git(root,"ls-files","--stage")==stages
            assert (root/"sibling").read_bytes()==b"sibling\r\nEOF"
            assert (root/"large.txt").read_bytes()==large.replace(b"line 09000 stable",b"line 09000 change")
            if action=="load-large":
                assert report["dense_limit_verified"]
                assert (root/"dense.txt").read_bytes()==b"dense worktree\r\nEOF"
                assert git(root,"show",":dense.txt")==dense
            assert all((files/f"{number:04}.txt").read_bytes()==content for number in range(3000))
            assert (root/"f").read_bytes()==(f if action in ("load-mass","load-large") else b"external result\n" if action=="load-draft" else b"other\n")
            after=len([item for item in git(root,"status","--porcelain=v1","-z","--untracked-files=all").split(b"\0") if item]);assert report["final_files"]==after
            assert report["measurements"]["cache"]["hits"]>0
            assert report["measurements"]["maximum_file_rows_per_batch"]<200
            assert report["measurements"]["next_frame_callback_wall"]["count"]>0
            reports.append(report)
        destination.write_text(json.dumps({"ok":True,"runs":reports,"scope":"real Wayland handlers/virtual batches; backend/preparation/next-frame wall/RSS; no isolated GPU execution or physical hardware claim"},indent=2,ensure_ascii=False)+"\n")
        print(f"Native load/event smoke: {len(reports)} scenarios passed")


if __name__=="__main__":
    smoke(Path(sys.argv[1]))
