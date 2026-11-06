"""Explicit, resumable release. Run only on trusted version tags, never PR code.

All destinations are checked before the first write. Archive comparisons include
source contents and modes, not just a version label. No credentials enter URLs.
"""
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile
import tempfile
import tomllib

VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
TAP = "Narqulie/homebrew-stickynote"


def run(*args, cwd=None, env=None):
    # Never echo argv or secret-bearing environment. Child tools get only the
    # credential they need; build scripts do not inherit publication secrets.
    clean = {k: v for k, v in os.environ.items() if k not in {
        "GH_TOKEN", "GITHUB_TOKEN", "CARGO_REGISTRY_TOKEN", "HOMEBREW_TAP_PAT"
    }}
    clean.update(env or {})
    return subprocess.check_output(args, cwd=cwd, env=clean)


def gh(*args):
    return run("gh", *args, env={"GH_TOKEN": os.environ["GH_TOKEN"]})


def archive_entries(data):
    entries = {}
    root = None
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as archive:
        for item in archive.getmembers():
            path = PurePosixPath(item.name)
            if path.is_absolute() or ".." in path.parts or not path.parts:
                raise ValueError("Unsafe archive path")
            root = root or path.parts[0]
            if path.parts[0] != root:
                raise ValueError("Archive has multiple roots")
            if item.isdir():
                continue
            name = str(PurePosixPath(*path.parts[1:]))
            if name == "." or name in entries:
                raise ValueError("Duplicate or root archive entry")
            if item.isfile():
                entries[name] = ("file", item.mode & 0o777, archive.extractfile(item).read())
            elif item.issym():
                entries[name] = ("symlink", item.mode & 0o777, item.linkname.encode())
            else:
                raise ValueError("Unsupported archive entry")
    if not entries:
        raise ValueError("Empty archive")
    return entries


def verify_archive(actual, expected):
    if archive_entries(actual) != archive_entries(expected):
        raise ValueError("Existing/downloaded archive differs from selected source")


def verify_crate(actual, expected, commit):
    entries = archive_entries(actual)
    vcs = json.loads(entries[".cargo_vcs_info.json"][2])
    if vcs["git"]["sha1"] != commit or vcs["git"].get("dirty", False):
        raise ValueError("Existing crate has mismatched source commit")
    verify_archive(actual, expected)


def formula_state(text, version, url, checksum):
    urls = re.findall(r'^  url "([^"]+)"$', text, re.M)
    sums = re.findall(r'^  sha256 "([0-9a-f]{64})"$', text, re.M)
    if len(urls) != 1 or len(sums) != 1:
        raise ValueError("Unexpected tap formula format")
    old = re.fullmatch(
        r"https://github.com/Narqulie/stickynote/archive/refs/tags/v([0-9]+\.[0-9]+\.[0-9]+)\.tar\.gz",
        urls[0], re.I,
    )
    if not old:
        raise ValueError("Unexpected tap source URL")
    if old[1] == version:
        if urls[0] != url or sums[0] != checksum:
            raise ValueError("Tap version exists with mismatched source/checksum")
        return True
    if tuple(map(int, old[1].split("."))) > tuple(map(int, version.split("."))):
        raise ValueError("Refusing to downgrade the tap")
    return False


def download(url, path, missing_ok=False):
    args = ["curl", "--silent", "--show-error", "--location", "--retry", "5",
            "--retry-all-errors", "--connect-timeout", "15", "--max-time", "180",
            "--output", str(path), "--write-out", "%{http_code}"]
    if not missing_ok:
        args.append("--fail")
    status = run(*args, url).decode()
    if missing_ok and status == "404":
        return False
    if status != "200":
        raise ValueError(f"Download failed with HTTP {status}")
    return True


def release_metadata(repo, tag):
    result = subprocess.run(
        ["gh", "api", f"repos/{repo}/releases/tags/{tag}"],
        env={k: v for k, v in os.environ.items() if k not in {
            "CARGO_REGISTRY_TOKEN", "HOMEBREW_TAP_PAT", "GITHUB_TOKEN"
        }}, capture_output=True, check=False,
    )
    if result.returncode == 0:
        return json.loads(result.stdout)
    if re.search(rb"\(HTTP 404\)", result.stderr):
        return None
    raise ValueError("GitHub release lookup failed (not a confirmed 404)")


def main():
    version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    if not VERSION.fullmatch(version):
        raise ValueError("This workflow supports stable numeric X.Y.Z releases only")
    tag = "v" + version
    event = os.environ["GITHUB_EVENT_NAME"]
    if event == "workflow_dispatch":
        if os.environ["REQUESTED_VERSION"] != version:
            raise ValueError("Requested version differs from Cargo.toml")
    elif event != "push" or os.environ["GITHUB_REF"] != "refs/tags/" + tag:
        raise ValueError("Publication requires the exact version tag or manual version")
    commit = run("git", "rev-parse", "HEAD").decode().strip()
    if run("git", "rev-parse", tag + "^{commit}").decode().strip() != commit:
        raise ValueError("Tag does not identify checked-out source")
    repo = os.environ["GITHUB_REPOSITORY"]
    if repo.lower() != "narqulie/stickynote":
        raise ValueError("Unexpected publication repository")
    remote = run("git", "ls-remote", "origin", "refs/tags/" + tag, "refs/tags/" + tag + "^{}").decode().splitlines()
    targets = dict(line.split()[::-1] for line in remote)
    if targets.get("refs/tags/" + tag + "^{}", targets.get("refs/tags/" + tag)) != commit:
        raise ValueError("Remote tag differs from selected source")
    if run("git", "status", "--porcelain").strip():
        raise ValueError("Release checkout must be clean")

    with tempfile.TemporaryDirectory() as temp:
        scratch = Path(temp)
        # Cargo package embeds exact git identity and the locked package sources.
        run("cargo", "package", "--locked", "--no-verify")
        crate = Path("target/package") / f"stickynote-{version}.crate"
        expected_crate = crate.read_bytes()
        crate_exists = download(f"https://crates.io/api/v1/crates/stickynote/{version}", scratch / "crate.json", True)
        if crate_exists:
            download(f"https://static.crates.io/crates/stickynote/stickynote-{version}.crate", scratch / "existing.crate")
            verify_crate((scratch / "existing.crate").read_bytes(), expected_crate, commit)

        metadata = release_metadata(repo, tag)
        if metadata is not None and (metadata["tag_name"] != tag or metadata["draft"] or metadata["prerelease"]):
            raise ValueError("Existing GitHub release has mismatched state")
        # GitHub source archives are tied to the independently checked remote tag.
        url = f"https://github.com/Narqulie/stickynote/archive/refs/tags/{tag}.tar.gz"
        download(url, scratch / "source.tar.gz")
        archive = (scratch / "source.tar.gz").read_bytes()
        verify_archive(archive, run("git", "archive", "--format=tar", "--prefix=source/", commit))
        checksum = hashlib.sha256(archive).hexdigest()
        tap = scratch / "tap"
        run("git", "clone", "--depth=1", "https://github.com/" + TAP + ".git", str(tap))
        formula = tap / "Formula/stickynote.rb"
        text = formula.read_text()
        tap_exists = formula_state(text, version, url, checksum)
        print(f"Verified source {commit}; completed destinations: crates={crate_exists}, GitHub={metadata is not None}, tap={tap_exists}")

        if not crate_exists:
            # The identical commit already passed the locked CI matrix. Avoid
            # running dependency build scripts with the registry token present.
            run("cargo", "publish", "--locked", "--no-verify", env={"CARGO_REGISTRY_TOKEN": os.environ["CARGO_REGISTRY_TOKEN"]})
        if metadata is None:
            gh("release", "create", tag, "--repo", repo, "--verify-tag", "--title", tag, "--generate-notes")
        if not tap_exists:
            text = re.sub(r'^  url "[^"]+"$', f'  url "{url}"', text, flags=re.M)
            text = re.sub(r'^  sha256 "[^"]+"$', f'  sha256 "{checksum}"', text, flags=re.M)
            # Repair legacy formula tests while updating the selected release.
            text = text.replace('stickynote --help', 'stickynote --version')
            formula.write_text(text)
            run("ruby", "-c", str(formula))
            run("git", "config", "user.name", "github-actions[bot]", cwd=tap)
            run("git", "config", "user.email", "github-actions[bot]@users.noreply.github.com", cwd=tap)
            run("git", "add", "Formula/stickynote.rb", cwd=tap)
            run("git", "commit", "-m", "stickynote " + version, cwd=tap)
            # Askpass keeps the PAT out of argv, remote URLs and git config.
            askpass = scratch / "askpass.sh"
            askpass.write_text('#!/bin/sh\ncase "$1" in *Username*) printf "%s\\n" x-access-token ;; *) printf "%s\\n" "$HOMEBREW_TAP_PAT" ;; esac\n')
            askpass.chmod(0o700)
            run("git", "-c", "credential.helper=", "push", cwd=tap, env={
                "GIT_ASKPASS": str(askpass), "GIT_TERMINAL_PROMPT": "0",
                "HOMEBREW_TAP_PAT": os.environ["HOMEBREW_TAP_PAT"],
            })
        print("Release destinations complete. Reruns verify identity before skipping.")


if __name__ == "__main__":
    main()
