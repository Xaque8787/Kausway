# Application integration example

See [the integration guide](../../docs/integrations/application-suite.md).

`application.py` is a minimal terminal application demonstrating the same process
ownership and Configure Kausway action that a desktop application would use.
`kausway.py` is a dependency-free reference helper for private protocol v1.

Build the suite locally, then run the example with explicit app-owned paths:

```sh
cargo build --manifest-path rust/Cargo.toml -p kausway-manager -p kausway-host -p kausway-admin --bins --locked
python3 examples/application-integration/application.py \
  --manager rust/target/debug/kausway \
  --config /path/to/application/config/kausway/config.toml \
  --data-root /path/to/application/data/kausway
```

The three executables must be siblings or supplied through manager binary overrides.
Adjust the build directory when `CARGO_TARGET_DIR` is set. Linux is validated;
Windows manager support/native macOS validation remain deferred.

Never log request/reply frames, setup URLs or credentials. For a server/container,
use the helper without opening a browser and expose the entry only through an
appropriate private local application action. No Docker packaging is added here.
