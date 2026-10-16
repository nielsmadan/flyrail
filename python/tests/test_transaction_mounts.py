import sys
from pathlib import Path

import pytest

import flyrail._state as state
from flyrail._state import shared_mount

ROOT = "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw"
DATA = "30 22 8:2 / /data rw,relatime shared:2 - ext4 /dev/sda2 rw"


def _info(*lines: str) -> str:
    return "\n".join(lines) + "\n"


def test_paths_on_one_mount_share_it() -> None:
    mountinfo = _info(ROOT, DATA)
    assert shared_mount(mountinfo, Path("/data/u/.state/staging"), Path("/data/u/project"))


def test_paths_on_different_filesystems_do_not_share_a_mount() -> None:
    mountinfo = _info(ROOT, DATA)
    assert not shared_mount(mountinfo, Path("/data/u/.state/staging"), Path("/srv/project"))


def test_bind_mounts_of_one_filesystem_do_not_share_a_mount() -> None:
    mountinfo = _info(
        ROOT,
        DATA,
        "41 30 8:2 /u/.state /work/state rw,relatime - ext4 /dev/sda2 rw",
        "42 30 8:2 /u/project /work/project rw,relatime - ext4 /dev/sda2 rw",
    )
    assert not shared_mount(mountinfo, Path("/work/state/staging"), Path("/work/project"))
    assert shared_mount(mountinfo, Path("/work/project/a"), Path("/work/project/b"))


def test_an_escaped_mount_point_is_decoded() -> None:
    mountinfo = _info(ROOT, DATA, "43 30 8:2 /x /data/u/my\\040state rw - ext4 /dev/sda2 rw")
    assert not shared_mount(mountinfo, Path("/data/u/my state/staging"), Path("/data/u/project"))
    assert shared_mount(mountinfo, Path("/data/u/my state/staging"), Path("/data/u/my state/p"))


def test_the_topmost_of_stacked_mounts_is_used() -> None:
    mountinfo = _info(ROOT, DATA, "44 30 8:3 / /data rw - tmpfs tmpfs rw")
    assert not shared_mount(mountinfo, Path("/data/u/staging"), Path("/"))
    assert shared_mount(mountinfo, Path("/data/u/staging"), Path("/data/v"))


@pytest.mark.parametrize(
    "mountinfo",
    [
        "",
        "garbage\n",
        _info(ROOT, "30 22 8:2 / /data rw,relatime shared:2 ext4 /dev/sda2 rw"),
        _info(ROOT, "x 22 8:2 / /data rw - ext4 /dev/sda2 rw"),
        _info(ROOT, "30 22 8-2 / /data rw - ext4 /dev/sda2 rw"),
        _info(ROOT, "", DATA),
        _info(ROOT, "30 22 8:2 / home rw - ext4 /dev/sda2 rw"),
    ],
)
def test_doubtful_mount_information_shares_no_mount(mountinfo: str) -> None:
    assert not shared_mount(mountinfo, Path("/data/u/.state/staging"), Path("/data/u/project"))


def test_paths_outside_every_listed_mount_share_no_mount() -> None:
    assert not shared_mount(_info(DATA), Path("/srv/state"), Path("/srv/project"))


def test_unreadable_mount_information_shares_no_mount(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def unreadable(path: Path) -> bytes:
        raise PermissionError(13, "denied", str(path))

    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setattr(Path, "read_bytes", unreadable)
    assert state._mountinfo() == ""
    monkeypatch.undo()
    monkeypatch.setattr(state, "_mountinfo", lambda: "")
    assert not state._same_mount(tmp_path, tmp_path)
