"""Guard the manual-only publication boundary and independent development CI."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PublicationWorkflowTests(unittest.TestCase):
    def test_only_manual_main_publisher_has_remote_write_permissions(self):
        workflows = ROOT / '.github/workflows'
        self.assertFalse((workflows / 'binaries.yml').exists())
        self.assertFalse((workflows / 'container-suite.yml').exists())
        for path in workflows.glob('*.yml'):
            text = path.read_text()
            if path.name != 'publish-release.yml':
                self.assertNotRegex(text, r'(contents|packages):\s*write', path.name)
                self.assertNotIn('docker/login-action', text, path.name)
        publisher = (workflows / 'publish-release.yml').read_text()
        trigger = publisher.split('on:', 1)[1].split('\npermissions:', 1)[0]
        self.assertIn('workflow_dispatch:', trigger)
        self.assertNotRegex(trigger, r'\n\s+(push|pull_request|workflow_run):')
        self.assertIn("if: github.ref == 'refs/heads/main'", publisher)
        self.assertIn('group: release-publication\n  cancel-in-progress: false', publisher)
        self.assertNotIn('secrets.', publisher)

    def test_jobs_use_pinned_checkouts_and_independent_tracks(self):
        text = (ROOT / '.github/workflows/publish-release.yml').read_text()
        self.assertEqual(text.count('uses: actions/checkout@v5'), text.count('ref: ${{ github.sha }}'))
        suite = text.split('\n  publish_suite:', 1)[1].split('\n  publish_client:', 1)[0]
        client = text.split('\n  publish_client:', 1)[1].split('\n  container_build:', 1)[0]
        self.assertIn('needs: preflight', suite)
        self.assertIn('needs: preflight', client)
        container = text.split('\n  container_build:', 1)[1].split('\n  summary:', 1)[0]
        self.assertNotIn('publish_client', container)
        self.assertEqual(container.count('!cancelled()'), 2)
        self.assertIn('container_needs_suite', container)
        self.assertIn('outputs: type=oci,', container)
        self.assertNotIn('push: true', container)
        self.assertLess(container.index('publish-release.py test-image'), container.index('publish-release.py push-image'))


if __name__ == '__main__':
    unittest.main()
