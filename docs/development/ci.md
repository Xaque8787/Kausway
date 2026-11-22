# Branch-independent CI and development artifacts

Push to any branch or open/update a pull request. **Build Validation** runs the
required tests and development builds, including on main. A branch need not be
named `work`. GitHub normally validates the final commit of a push, not every
intermediate commit pushed together.

There are initially no path-based exclusions: even documentation-only pushes run
the complete graph. Development runs can cancel superseded runs on the same ref;
active main runs are not canceled by newer pushes. GitHub concurrency is not a FIFO
queue, so pending main runs can still be superseded. Missing/canceled evidence
never qualifies a commit for publication.

## Required graph and summary

`scripts/ci/validation-policy.json` explicitly lists:

- Rust formatting, lint, tests and suite integration.
- Minimum supported Rust compiler check.
- Jellyfin plugin build/tests and development packaging.
- Combined Jellyfin image build/smoke test.
- Linux AMD64/ARM64 and Android ARM64/ARMv7 binary builds/packages.
- Android SDK, independent consumer and real-host emulator checks.
- AMD64/ARM64 suite containers and Alpine/Ubuntu embedding tests.
- Complete suite/client release candidate inventories and payload validation.

The parent workflow calls reusable binary, Android and container workflows. The
container job reuses the verified Linux binary archives from the same run instead
of compiling the suite again. A failed-jobs rerun can use a successful binary
artifact from an earlier attempt of that same run/source; attempts remain separate
and the newest verified available attempt is selected explicitly.

The stable **Validation summary** job runs with `if: always()` after all required
jobs. Every required result must be `success`: failure, cancellation, skip or a
missing result fails the summary. There are no inferred skip exceptions. Changes
to required jobs must update both the workflow graph and policy; contract tests
check that they agree.

The summary artifact is named:

```text
validation-summary-<full-sha>-run<id>-attempt<number>
```

Its JSON records workflow identity, exact SHA/ref/event/run/attempt, all required
outcomes and overall success. Failed summaries also upload evidence when possible.
An absent report is not success. If the entire run is canceled, an earlier green
job or saved report does not make that run successful.

The manual publisher queries the expected trusted main workflow run for the
candidate SHA and its applicable completed attempt, verify overall run success,
and inspect this policy/evidence. It must not trust a similarly named check from
another workflow or merely accept an artifact filename. PR checks use GitHub's
synthetic merge SHA; they do not replace validation of the resulting main commit.
The summary check is the intended stable branch-protection entry once configured;
this change does not edit repository protection settings.

## Download artifacts

Open **Actions → Build Validation → desired commit → Artifacts**. Artifact names
include commit and run/attempt identity. Binary groups additionally identify suite
development version and target; manifests include each binary's actual track version.

| Artifact prefix | Contents |
| --- | --- |
| `development-kausway-...-<target>-run...-attempt...` | Suite/component `.tar.gz` archives, notices, manifests and checksums |
| `android-sdk-development-...` | AAR/POM, native libraries, sample APK and provenance; uploaded before emulator testing |
| `kausway-android-sdk-...` | SDK candidate files uploaded only after emulator success |
| `development-jellyfin-plugin-...` | Plugin ZIP, checksum inventory and provenance |
| `development-suite-image-amd64-...` / `arm64-...` | Tested Docker image archive, checksum and provenance |
| `validation-summary-...` | Aggregate validation evidence |
| `release-candidate-suite-...` / `release-candidate-client-...` | Complete verified release-layout candidates; not published releases |

The existence of a build artifact is not proof the whole validation passed. Check
the summary, especially for early SDK artifacts. Diagnostics/corrected-lock/format
artifacts are labeled separately and identify their run too. Typical build artifacts
are retained for 14 days; summaries for 30 days, subject to repository settings.

Binary `BUILD-INFO.json` includes `ci`; SDK/plugin/image payloads include a
`BUILD-INFO.<kind>.json` with CI identity, versions, target, dirty-source flag and
per-file hashes. Actions archive digests and the payload's checksums serve different
layers. Use the payload checksum files after extracting the downloaded Actions ZIP.

## Test an image locally

Download the image matching your CPU, extract the Actions ZIP, then:

```sh
sha256sum -c kausway-suite-amd64.docker.tar.gz.sha256
docker load -i kausway-suite-amd64.docker.tar.gz
```

The loaded name is `kausway-suite:test`. A Compose service can use:

```yaml
image: kausway-suite:test
pull_policy: never
```

These are single-platform Docker archives, not pullable registry tags. Load the
archive separately on each Docker engine. No development image is pushed to GHCR.

To build locally from a downloaded **8.2-or-newer binary artifact**, extract its
ZIP into a directory, use the matching source checkout and full SHA, then:

```sh
python3 scripts/packaging/stage-container-artifact.py \
  --artifacts /path/to/extracted-binary-artifact \
  --target x86_64-unknown-linux-musl --commit <full-source-sha> \
  --output /tmp/kausway-image-context
docker build -f deploy/standalone/runtime.Dockerfile \
  --build-arg VCS_REF=<full-source-sha> \
  -t kausway-suite:local /tmp/kausway-image-context
```

The helper verifies checksums, payload inventory, source, versions, architecture,
static linkage and notices before staging. In Actions it additionally requires the
same repository/ref/event/run and a valid current-or-earlier attempt. Dirty-source
artifacts are rejected by default. `--allow-dirty` exists for explicit local testing
of uncommitted builds and is prohibited in Actions.

Source builds remain available through `deploy/standalone/Dockerfile`; see the
[Docker guide](../integrations/docker-suite.md). Neither local path publishes images.

## Manual development builds

**Development binaries**, **Android SDK**, and **Development containers** also have
manual dispatch. Select the branch/tag to build. A standalone manual container run
builds from repository source; calls from Build Validation use the same-run binary
artifacts. Component-only runs do not replace the parent validation summary.

GitHub generally requires a workflow to exist on the default branch before showing
its Run workflow button. Automatic parent calls work on development branches before
that merge. Use the automatic Build Validation artifacts in the meantime.

## Publication boundary

These development workflows have read-only repository permissions and no registry
login/push or GitHub Release steps. **Publish release** is the only publisher:
manual dispatch on main, with selected tracks and successful exact-commit main
validation. The legacy Binaries and container publishers are removed. Main pushes
and tag pushes produce no final publication. See [publication and recovery](publishing.md).

Real-phone/media-server acceptance testing remains a documented manual exercise,
not an additional approval or checklist gate in this CI graph.
