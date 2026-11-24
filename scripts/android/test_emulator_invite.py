import base64
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('emulator_invite', Path(__file__).with_name('rewrite-emulator-invite.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def encode(doc):
    return module.PREFIX + base64.urlsafe_b64encode(json.dumps(doc).encode()).decode().rstrip('=')


def decode(link):
    payload = link[len(module.PREFIX):]
    return json.loads(base64.urlsafe_b64decode(payload + '=' * (-len(payload) % 4)))


class EmulatorInvitationTests(unittest.TestCase):
    def test_preserves_identity_token_and_all_real_routes(self):
        original = {'v': 1, 'token': 'fixture-token', 'addr': {'id': 'fixture-host', 'addrs': [
            {'Ip': '[fd00::1]:4000'}, {'Ip': '10.1.0.4:5000'}, {'Ip': '172.17.0.1:5000'},
        ]}}
        rewritten = decode(module.emulator_invitation(encode(original)))
        self.assertEqual(rewritten['v'], original['v'])
        self.assertEqual(rewritten['token'], original['token'])
        self.assertEqual(rewritten['addr']['id'], original['addr']['id'])
        self.assertEqual(rewritten['addr']['addrs'], original['addr']['addrs'] + [{'Ip': '10.0.2.2:5000'}])

    def test_alias_is_not_duplicated(self):
        original = {'v': 1, 'token': 'fixture-token', 'addr': {'id': 'fixture-host', 'addrs': [{'Ip': '10.0.2.2:5000'}]}}
        link = module.emulator_invitation(encode(original))
        self.assertEqual(decode(module.emulator_invitation(link)), original)

    def test_adds_alias_for_every_ipv4_port_without_rewriting_real_routes(self):
        original = {'v': 1, 'token': 'fixture-token', 'addr': {'id': 'fixture-host', 'addrs': [
            {'Ip': '127.0.0.1:4000'}, {'Ip': '10.1.0.4:5000'}, {'Ip': '172.17.0.1:5000'},
            {'Ip': '[fd00::1]:6000'},
        ]}}
        rewritten = decode(module.emulator_invitation(encode(original)))
        self.assertEqual(rewritten['addr']['addrs'], original['addr']['addrs'] + [
            {'Ip': '10.0.2.2:4000'}, {'Ip': '10.0.2.2:5000'},
        ])
        self.assertEqual(rewritten['token'], original['token'])
        self.assertEqual(decode(module.emulator_invitation(encode(rewritten))), rewritten)

    def test_existing_alias_does_not_hide_a_second_ipv4_port(self):
        original = {'v': 1, 'token': 'fixture-token', 'addr': {'id': 'fixture-host', 'addrs': [
            {'Ip': '10.0.2.2:4000'}, {'Ip': '10.1.0.4:5000'},
        ]}}
        rewritten = decode(module.emulator_invitation(encode(original)))
        self.assertEqual(rewritten['addr']['addrs'], original['addr']['addrs'] + [{'Ip': '10.0.2.2:5000'}])

    def test_reports_missing_ipv4_without_exposing_invite(self):
        original = {'v': 1, 'token': 'fixture-token', 'addr': {'id': 'fixture-host', 'addrs': [{'Ip': '[fd00::1]:4000'}]}}
        with self.assertRaisesRegex(ValueError, 'advertised IPv4'):
            module.emulator_invitation(encode(original))
