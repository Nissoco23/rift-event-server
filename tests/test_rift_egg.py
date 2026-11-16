"""The Pterodactyl egg embeds its install script: the committed egg must match what tools/make_egg.py writes."""
import json
import pathlib
import subprocess
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


class RiftEgg(unittest.TestCase):
    def test_the_committed_egg_is_what_the_generator_writes(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools" / "make_egg.py"), "--check"],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_the_egg_asks_for_the_variables_the_server_reads_and_stops_with_stop(self):
        egg = json.loads((ROOT / "packaging" / "rift" / "pterodactyl-egg.json").read_text())
        names = {v["env_variable"] for v in egg["variables"]}
        # init reads these (crates/horizon-server/src/init.rs); the install script reads RIFT_VERSION.
        self.assertTrue(
            {"HORIZON_EULA", "HORIZON_ONLINE", "HORIZON_WHITELIST", "HORIZON_PLAYERS",
             "HORIZON_OP", "HORIZON_WORLD", "HORIZON_SEED", "RIFT_VERSION"} <= names
        )
        self.assertEqual(egg["config"]["stop"], "stop")
        self.assertEqual(egg["startup"], "sh ./start.sh")


if __name__ == "__main__":
    unittest.main()
