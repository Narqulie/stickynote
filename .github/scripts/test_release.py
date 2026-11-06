import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import release


COMMIT = "a" * 40


def archive(files, root="source"):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as tar:
        for name, data in files.items():
            info = tarfile.TarInfo(root + "/" + name)
            info.size = len(data)
            info.mode = 0o644
            tar.addfile(info, io.BytesIO(data))
    return output.getvalue()


class ReleaseTests(unittest.TestCase):
    def test_archive_requires_exact_source_not_only_a_version_or_successful_http(self):
        good = archive({"Cargo.toml": b'version = "0.3.2"', "src/main.rs": b"good"})
        release.verify_archive(good, archive({"Cargo.toml": b'version = "0.3.2"', "src/main.rs": b"good"}, "other-root"))
        with self.assertRaises(ValueError):
            release.verify_archive(archive({"Cargo.toml": b'version = "0.3.2"', "src/main.rs": b"different"}), good)
        with self.assertRaises(tarfile.ReadError):
            release.verify_archive(b"HTTP error page", good)
        with self.assertRaises(ValueError):
            release.archive_entries(archive({"../escape": b"bad"}))

    def test_crate_requires_commit_clean_vcs_info_and_all_package_contents(self):
        vcs = json.dumps({"git": {"sha1": COMMIT}, "path_in_vcs": ""}).encode()
        good = archive({".cargo_vcs_info.json": vcs, "src/main.rs": b"good"})
        release.verify_crate(good, good, COMMIT)
        with self.assertRaises(ValueError):
            release.verify_crate(good, good, "b" * 40)
        with self.assertRaises(ValueError):
            release.verify_crate(archive({".cargo_vcs_info.json": vcs, "src/main.rs": b"bad"}), good, COMMIT)
        with self.assertRaises(ValueError):
            release.verify_crate(archive({".cargo_vcs_info.json": json.dumps({"git": {"sha1": COMMIT, "dirty": True}}).encode()}), good, COMMIT)

    def test_tap_skip_requires_url_and_checksum_identity_and_never_downgrades(self):
        url = "https://github.com/Narqulie/stickynote/archive/refs/tags/v0.3.2.tar.gz"
        text = f'  url "{url}"\n  sha256 "{"a" * 64}"\n'
        self.assertTrue(release.formula_state(text, "0.3.2", url, "a" * 64))
        self.assertFalse(release.formula_state(text.replace("v0.3.2", "v0.3.1"), "0.3.2", url, "a" * 64))
        for wrong in [text.replace("a" * 64, "b" * 64), text.replace("v0.3.2", "v0.3.3"), text.replace("Narqulie/stickynote", "attacker/source")]:
            with self.assertRaises(ValueError):
                release.formula_state(wrong, "0.3.2", url, "a" * 64)

    def test_download_retries_fails_and_does_not_treat_server_errors_as_absence(self):
        with patch.object(release, "run", return_value=b"200") as tool:
            self.assertTrue(release.download("https://example.invalid/source", Path("temp")))
            self.assertIn("--fail", tool.call_args.args)
            self.assertIn("--retry-all-errors", tool.call_args.args)
        with patch.object(release, "run", return_value=b"404"):
            self.assertFalse(release.download("url", Path("temp"), True))
        with patch.object(release, "run", return_value=b"503"):
            with self.assertRaises(ValueError):
                release.download("url", Path("temp"), True)

    def test_github_only_confirmed_404_is_absent(self):
        with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 1, b"", b"gh: Not Found (HTTP 404)")):
            self.assertIsNone(release.release_metadata("repo", "v0.3.2"))
        for error in [b"HTTP 500", b"Bad credentials (HTTP 401)", b"network failure"]:
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 1, b"", error)):
                with self.assertRaises(ValueError):
                    release.release_metadata("repo", "v0.3.2")

    def test_child_builds_do_not_inherit_publication_credentials(self):
        with patch.dict(os.environ, {"GH_TOKEN": "github-secret", "CARGO_REGISTRY_TOKEN": "cargo-secret", "HOMEBREW_TAP_PAT": "tap-secret"}):
            with patch.object(subprocess, "check_output", return_value=b"") as tool:
                release.run("cargo", "package")
                env = tool.call_args.kwargs["env"]
                self.assertNotIn("GH_TOKEN", env)
                self.assertNotIn("CARGO_REGISTRY_TOKEN", env)
                self.assertNotIn("HOMEBREW_TAP_PAT", env)
                release.run("cargo", "publish", env={"CARGO_REGISTRY_TOKEN": "cargo-secret"})
                env = tool.call_args.kwargs["env"]
                self.assertEqual(env["CARGO_REGISTRY_TOKEN"], "cargo-secret")
                self.assertNotIn("HOMEBREW_TAP_PAT", env)

    def test_partial_release_resumes_without_republishing_and_mismatch_blocks_all_writes(self):
        old_cwd = Path.cwd()
        state = {"crate": False, "github": False, "tap": False, "fail_github": True}
        writes = []
        source = archive({"Cargo.toml": b"source"})
        crate = archive({".cargo_vcs_info.json": json.dumps({"git": {"sha1": COMMIT}}).encode()})
        checksum = release.hashlib.sha256(source).hexdigest()
        url = "https://github.com/Narqulie/stickynote/archive/refs/tags/v0.3.2.tar.gz"

        def tool(*args, **kwargs):
            if args[:2] == ("git", "rev-parse"):
                return COMMIT.encode()
            if args[:2] == ("git", "ls-remote"):
                return f"{COMMIT}\trefs/tags/v0.3.2\n".encode()
            if args[:2] == ("git", "archive"):
                return source
            if args[:2] == ("cargo", "package"):
                target = Path("target/package")
                target.mkdir(parents=True, exist_ok=True)
                (target / "stickynote-0.3.2.crate").write_bytes(crate)
            if args[:2] == ("git", "clone"):
                formula = Path(args[-1]) / "Formula/stickynote.rb"
                formula.parent.mkdir(parents=True)
                formula.write_text(f'  url "{url if state["tap"] else url.replace("0.3.2", "0.3.1")}"\n  sha256 "{checksum if state["tap"] else "b" * 64}"\n')
            if args[:2] == ("cargo", "publish"):
                writes.append("crate")
                state["crate"] = True
            if "push" in args:
                writes.append("tap")
                state["tap"] = True
            return b""

        def fetch(url, path, missing_ok=False):
            if missing_ok:
                return state["crate"]
            path.write_bytes(crate if url.endswith(".crate") else source)
            return True

        def github(*args):
            writes.append("github")
            if state["fail_github"]:
                state["fail_github"] = False
                raise RuntimeError("interrupted after crates.io")
            state["github"] = True

        def metadata(*args):
            return {"tag_name": "v0.3.2", "draft": False, "prerelease": False} if state["github"] else None

        with tempfile.TemporaryDirectory() as temp:
            try:
                os.chdir(temp)
                Path("Cargo.toml").write_text('[package]\nversion = "0.3.2"\n')
                env = {"GITHUB_EVENT_NAME": "workflow_dispatch", "REQUESTED_VERSION": "0.3.2", "GITHUB_REPOSITORY": "Narqulie/stickynote", "CARGO_REGISTRY_TOKEN": "not-real", "HOMEBREW_TAP_PAT": "not-real"}
                with patch.dict(os.environ, env), patch.object(release, "run", side_effect=tool), patch.object(release, "gh", side_effect=github), patch.object(release, "download", side_effect=fetch), patch.object(release, "release_metadata", side_effect=metadata):
                    with self.assertRaises(RuntimeError):
                        release.main()
                    release.main()
                    self.assertEqual(writes, ["crate", "github", "github", "tap"])
                    release.main()
                    self.assertEqual(writes, ["crate", "github", "github", "tap"])
                    # A mismatched completed destination must fail before any new write.
                    state["github"] = False
                    with patch.object(release, "verify_crate", side_effect=ValueError("source mismatch")):
                        with self.assertRaises(ValueError):
                            release.main()
                    self.assertEqual(writes, ["crate", "github", "github", "tap"])
                    with patch.object(release, "release_metadata", return_value={"tag_name": "v0.3.2", "draft": True, "prerelease": False}):
                        with self.assertRaises(ValueError):
                            release.main()
                    with patch.object(release, "formula_state", side_effect=ValueError("tap checksum mismatch")):
                        with self.assertRaises(ValueError):
                            release.main()
                    def moved_tag(*args, **kwargs):
                        if args[:2] == ("git", "ls-remote"):
                            return f'{"b" * 40}\trefs/tags/v0.3.2\n'.encode()
                        return tool(*args, **kwargs)
                    with patch.object(release, "run", side_effect=moved_tag):
                        with self.assertRaises(ValueError):
                            release.main()
                    self.assertEqual(writes, ["crate", "github", "github", "tap"])
                    os.environ["REQUESTED_VERSION"] = "0.3.9"
                    with self.assertRaises(ValueError):
                        release.main()
                    os.environ["GITHUB_EVENT_NAME"] = "pull_request"
                    with patch.object(release, "run") as tools:
                        with self.assertRaises(ValueError):
                            release.main()
                        tools.assert_not_called()
            finally:
                os.chdir(old_cwd)


if __name__ == "__main__":
    unittest.main()
