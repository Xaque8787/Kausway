# Release packaging (Phase 8.3)

Packaging creates no Git tags, GitHub Releases or GHCR publications. Manual
publication and release-state/API checks follow in 8.4. Existing legacy publishers
must not be used as the new release model.

## Product and asset boundaries

`scripts/releases/layout.py` declares the exact expected inventory; versions come
from `releases.toml`. Packaging does not bump or synchronize versions implicitly.

| Track | Required payloads |
| --- | --- |
| Suite | Linux AMD64/ARM64 suite + individual manager/host/admin archives; Android ARM64/ARMv7 host archives; stable-only Jellyfin ZIP |
| Client | CLI archives for all four targets; Android AAR, POM, sources JAR, notices archive and README |
| Container | Image descriptor recording index/platform digests, tags, selected suite and source identities; produced by the future publisher |

Suite archives/images contain only `kausway`, `kausway-host`, and `kausway-admin`.
The CLI is separate. Android has individual host/client downloads, not a combined
bundle. Corresponding combined/individual binaries must have identical hashes.
Native tests combine suite/client binaries only in a disposable test directory.

Stable suite candidates require the Jellyfin package; prereleases omit it while
still building/testing the plugin. Its development ZIP remains available for tests.
Plugin packages include project licenses. SDK sources are deduplicated/deterministic;
SDK notices cover the project and JNI dependencies for all three ABIs. Direct AAR
consumers must supply POM dependencies explicitly, as the SDK README explains.

## Candidate assembly

Build Validation calls **Release candidate packages** after binary/plugin/Android
validation. Its success is required by the final summary. Uploaded artifacts are
`release-candidate-suite-<sha>-run<id>-attempt<n>` and the corresponding client name.
Inputs must match the source and validation run. Earlier successful attempts of
that run can be reused. Ambiguous inputs, dirty CI sources, wrong targets/versions,
incomplete SDK/plugin payloads and checksum mismatches fail assembly.

Each candidate contains exactly its required payloads, adjacent `.sha256` files,
`RELEASE-MANIFEST.json` (track/version/source/provenance/hashes/sizes), and
`SHA256SUMS` covering payloads, sidecars and manifest. Verification checks payload
structure and static linkage as well as integrity. Source identity relies on trusted
CI/release retrieval; this does not introduce cryptographic signing.

Release filenames omit the development suffix. `release_candidate` and manifest
`state: candidate` describe preparation, not remote publication status. The same
bytes can be promoted unchanged; a published release page establishes publication.

For local assembly, extract all four binary artifact ZIPs plus the validated SDK
and plugin artifacts into separate subdirectories of one input folder:

```sh
python3 scripts/releases/package-candidate.py --track suite \
  --inputs /path/to/inputs --output /tmp/suite-candidate --commit <full-sha> \
  --validation-run-id <id> --validation-attempt <number>
python3 scripts/releases/package-candidate.py --track client \
  --inputs /path/to/inputs --output /tmp/client-candidate --commit <full-sha> \
  --validation-run-id <id> --validation-attempt <number>
python3 scripts/releases/package-candidate.py --verify-only --track suite \
  --output /tmp/suite-candidate --commit <full-sha>
```

Use the matching source checkout and new output directories. Do not flatten retry
artifacts onto one another. Explicit local `--allow-dirty` testing retains dirty
provenance and is forbidden in Actions. Local inputs without Actions identity can
omit the run/attempt pair; Actions assembly must specify both.

## Selected-suite container staging

Download the complete selected suite release into a directory, then:

```sh
python3 scripts/packaging/stage-container-release.py \
  --release /path/to/suite-release --target x86_64-unknown-linux-musl \
  --commit <container-source-sha> --output /tmp/container-context
docker build -f deploy/standalone/runtime.Dockerfile \
  --build-arg VCS_REF=<container-source-sha> -t kausway-suite:local /tmp/container-context
```

The helper reads `container.suite_version`, verifies its full inventory, and copies
the selected binaries/notices without rebuilding. Suite and container commits are
recorded separately alongside source asset/manifest digests. An older selected
suite is permitted; current workspace binaries are not substituted.

Legacy `v0.4.0` assets are not the new suite layout. The first new-layout release
must publish its selected suite before container publication. Update
`container.suite_version` explicitly during release preparation.

Container descriptor verification requires AMD64/ARM64 digests and one index for
version/commit tags, plus `latest` for stable releases only. Actual registry
inspection, publication, retries and rollback remain 8.4 responsibilities.

## Tests

Run the release and CI Python unittest suites. Packaging tests use non-executed
ELF/SDK fixtures for full-inventory and failure cases; native runtime and Android
CI use actual compiled artifacts. Phone login/streaming remains manual acceptance.
