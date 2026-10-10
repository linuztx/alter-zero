// npm/package.json's prepublishOnly (docs/npm.md): this directory is the
// launcher's source, never what gets published. `scripts/release.sh npm`
// stages a copy with the LICENSE beside it and without this hook, and
// publishes it only after the four platform packages its optional
// dependencies name — published from here, the launcher could reach the
// registry before them and install with no binary at all. A dry run
// publishes nothing, so it may look.
'use strict';

if (process.env.npm_config_dry_run !== 'true') {
  process.stderr.write(
    'This is the source of the npm launcher, not a package to publish.\n' +
      'Publish a release with scripts/release.sh npm VERSION DIST, which publishes\n' +
      'the four platform packages first and the launcher last (docs/npm.md).\n',
  );
  process.exit(1);
}
