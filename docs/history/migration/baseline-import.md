# Kausway baseline import

Source: https://github.com/Xaque8787/iroh-jellyfin

Branch: main

Commit: `e8a67d2f87a94915e1d85740f264e1b157f9e80b`

This baseline copies all tracked files from the source commit without changing
source code, directory layout, package identities, artifact names or workflows.
Kausway retains its own Git history. The original Kausway README is preserved in
[kausway-original-readme.md](kausway-original-readme.md).

Generated outputs, caches, credentials and runtime state are excluded by importing
from the Git tree rather than the source working directory. The original
repository remains intact. Existing validation reports describe the source
repository; Kausway workflow validation is a subsequent migration step.

Repository URLs, project-facing names, release configuration and directory
restructuring are intentionally deferred to subsequent migration changes.
