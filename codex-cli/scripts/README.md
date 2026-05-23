# npm releases

Use the staging helper in the repo root to generate npm tarballs for a release. For
example, to stage the Windows x64 Codepilot packages for version `0.1.0`:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.1.0 \
  --package codepilot
```

This downloads the required native package archive artifacts, hydrates `vendor/` for
each package, and writes tarballs to `dist/npm/`.

When `--package codepilot` is provided, the staging helper builds the lightweight
`@charzhu/codepilot` root package plus the Windows x64 native package
`@charzhu/codepilot-win32-x64`.

Direct `build_npm_package.py` invocations are still useful for package-specific
debugging, but native packages expect `--vendor-src` to point at a prehydrated
`vendor/` tree. Release packaging should use `scripts/stage_npm_packages.py`.
