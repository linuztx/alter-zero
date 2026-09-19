"""Replacement keeps resource constraints and refuses custom environment loss."""

import json
import unittest

import test_scripts as scripts


IMAGE_ID = "sha256:" + "a" * 64


def quoted_environment(*entries):
    return "\n".join(json.dumps(entry, ensure_ascii=False) for entry in entries)


class ReplacementConfigTests(scripts.ScriptCase):
    def settings(self, *records, pids="<nil>"):
        return "\n".join([
            "format|1", "image|rebuilt:kali", f"image-id|{IMAGE_ID}",
            'mount|volume|"kept-home"|"/root"|true',
            'mount|volume|"kept-work"|"/workspace"|true',
            'process|"/usr/bin/tini"|["--","sleep","infinity"]',
            "network|bridge", "restart|unless-stopped", "security|no-new-privileges",
            f"pids|{pids}", *records,
        ])

    def replace(self, *options, engine="podman", settings=None, **env):
        return self.script(scripts.RUN, "--engine", engine, "--replace", *options,
                           STUB_CONTAINER="1", STUB_INSPECT=settings or self.settings(), **env)

    def test_environment_baseline_uses_original_image_id_even_when_tag_is_rebuilt(self):
        original = quoted_environment("PATH=/old/bin", "IMAGE_DEFAULT=old")
        result, calls = self.replace(STUB_IMAGE_ENV=original, STUB_CONTAINER_ENV=original)
        self.assertEqual(result.returncode, 0, result.stderr)
        snapshots = [call["args"] for call in calls
                     if call["args"][:2] == ["image", "inspect"] and "--format" in call["args"]]
        self.assertEqual(len(snapshots), 1)
        self.assertEqual(snapshots[0][-1], IMAGE_ID)
        self.assertEqual(self.one(calls, "run")["args"][-1], "rebuilt:kali")

    def test_image_environment_order_and_literal_special_characters_do_not_matter(self):
        entries = ["PATH=/usr/bin", 'DEFAULT=a|b"c\\d\ne', "UNICODE=日本語"]
        result, calls = self.replace(STUB_IMAGE_ENV=quoted_environment(*entries),
                                    STUB_CONTAINER_ENV=quoted_environment(*reversed(entries)))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.one(calls, "run")

    def test_engine_root_and_hostname_defaults_are_allowed(self):
        for engine in ("docker", "podman"):
            with self.subTest(engine=engine):
                entries = ["PATH=/usr/bin", "HOME=/root", "HOSTNAME=az-kali"]
                if engine == "podman":
                    entries.append("container=podman")
                result, calls = self.replace(engine=engine, STUB_CONTAINER_ENV=quoted_environment(*entries))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.one(calls, "run")

    def test_added_or_changed_custom_environment_is_refused_without_leaking_values(self):
        secret = 'private-token|with"quotes\\and\nnewlines'
        for changed in (f"CUSTOM={secret}", f"PATH={secret}", "HOME=/elsewhere",
                        "HOSTNAME=other", "WAYLAND_DISPLAY=wayland-0"):
            for engine in ("docker", "podman"):
                with self.subTest(engine=engine, variable=changed.split("=", 1)[0]):
                    result, calls = self.replace(engine=engine,
                        STUB_CONTAINER_ENV=quoted_environment("PATH=/usr/bin", changed))
                    self.assertRefused(result, calls, "rm", "run", saying="custom environment")
                    self.assertNotIn("private-token", result.stdout + result.stderr)

    def test_removed_image_environment_is_refused(self):
        result, calls = self.replace(STUB_IMAGE_ENV=quoted_environment("PATH=/usr/bin", "IMAGE_DEFAULT=kept"),
                                    STUB_CONTAINER_ENV=quoted_environment("PATH=/usr/bin"))
        self.assertRefused(result, calls, "rm", "run", saying="removed image environment")

    def test_managed_clipboard_environment_requires_matching_mounts(self):
        managed = quoted_environment("PATH=/usr/bin", "WAYLAND_DISPLAY=wayland-0",
            "XDG_RUNTIME_DIR=/run/alter-zero", "DISPLAY=:12", "XAUTHORITY=/run/alter-zero/x11/Xauthority")
        records = [
            'mount|bind|"/host/wayland"|"/run/alter-zero/wayland-0"|false',
            'mount|bind|"/host/x11"|"/tmp/.X11-unix"|false',
            'mount|bind|"/host/auth"|"/run/alter-zero/x11"|false',
        ]
        # Disabling forwarding explicitly still allows inspection of the old
        # launcher's environment; it will not be copied to the replacement.
        result, calls = self.replace("--no-clipboard", settings=self.settings(*records),
                                    STUB_CONTAINER_ENV=managed)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("--env", self.one(calls, "run")["args"])
        for missing in range(len(records)):
            with self.subTest(missing_mount=missing):
                retained = records[:missing] + records[missing + 1:]
                result, calls = self.replace("--no-clipboard", settings=self.settings(*retained),
                                            STUB_CONTAINER_ENV=managed)
                self.assertRefused(result, calls, "rm", "run", saying="custom environment")

    def test_explicit_reset_skips_environment_comparison_and_repeats_custom_options(self):
        result, calls = self.replace("--reset-config", "--", "--env", "CUSTOM=repeated",
                                    "--cpus", "0.5", "--pids-limit", "42",
                                    STUB_CONTAINER_ENV=quoted_environment("CUSTOM=old"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any("--format" in call["args"] for call in calls))
        run_args = self.one(calls, "run")["args"]
        self.assertEqual(scripts.value_of(run_args, "--env"), ["CUSTOM=repeated"])
        self.assertEqual(scripts.value_of(run_args, "--cpus"), ["0.5"])
        self.assertEqual(scripts.value_of(run_args, "--pids-limit"), ["42"])

    def test_pid_limits_and_unlimited_values_are_inherited_on_both_engines(self):
        for engine in ("docker", "podman"):
            for limit in ("42", "2048", "0", "-1"):
                with self.subTest(engine=engine, limit=limit):
                    result, calls = self.replace(engine=engine, settings=self.settings(pids=limit))
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(scripts.value_of(self.one(calls, "run")["args"], "--pids-limit"), [limit])

    def test_unset_docker_pid_limit_remains_unset(self):
        for unset in ("<nil>", "<no value>", ""):
            with self.subTest(unset=unset):
                result, calls = self.replace(engine="docker", settings=self.settings(pids=unset))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("--pids-limit", self.one(calls, "run")["args"])

    def test_explicit_pid_limit_follows_the_inherited_limit(self):
        result, calls = self.replace("--", "--pids-limit", "84", settings=self.settings(pids="42"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(scripts.value_of(self.one(calls, "run")["args"], "--pids-limit"), ["42", "84"])


if __name__ == "__main__":
    unittest.main()
