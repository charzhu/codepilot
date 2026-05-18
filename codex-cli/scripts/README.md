# npm releases

Use the staging helper in the repo root to generate npm tarballs for a release. For
example, to stage the Windows x64 Codepilot packages for version `0.1.0`:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.1.0 \
  --package codepilot
```

This downloads the native artifacts once, hydrates `vendor/` for each package, and writes
tarballs to `dist/npm/`.

When `--package codepilot` is provided, the staging helper builds the lightweight
`@charzhu/codepilot` root package plus the Windows x64 native package
`@charzhu/codepilot-win32-x64`.

If you need to invoke `build_npm_package.py` directly, run
`codex-cli/scripts/install_native_deps.py` first and pass `--vendor-src` pointing to the
directory that contains the populated `vendor/` tree.
