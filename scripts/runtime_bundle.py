"""Produce deterministic compressed runtime archives and Rust include files."""
import gzip
import hashlib
import json
import tarfile


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_changed(path, value):
    if not path.exists() or path.read_text(encoding='utf-8') != value:
        path.write_text(value, encoding='utf-8')


def literal(value):
    marks = '#'
    while '"' + marks in str(value):
        marks += '#'
    return 'r' + marks + '"' + str(value) + '"' + marks


def write_bundle(stage, output, metadata):
    metadata = dict(metadata)
    files = [{'path': path.relative_to(stage).as_posix(), 'size': path.stat().st_size,
              'sha256': digest(path)} for path in sorted(stage.rglob('*')) if path.is_file()]
    metadata.update(schema_version=1, files=files)
    bundle = output / 'bundle.tar.gz'
    pending = output / 'bundle.tmp'
    with pending.open('wb') as raw:
        with gzip.GzipFile(filename='', fileobj=raw, mode='wb', mtime=0, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as archive:
                for item in files:
                    header = tarfile.TarInfo(item['path'])
                    header.size = item['size']
                    header.mode = 0o755 if item['path'] == metadata['executable'] else 0o644
                    with (stage / item['path']).open('rb') as data:
                        archive.addfile(header, data)
    if bundle.exists() and digest(pending) == digest(bundle):
        pending.unlink()
    else:
        pending.replace(bundle)
    metadata.update(bundle_sha256=digest(bundle), bundle_bytes=bundle.stat().st_size)
    manifest = output / 'manifest.json'
    write_changed(manifest, json.dumps(metadata, indent=2) + '\n')
    write_changed(output / 'embed.rs',
                  'pub const DATA: &[u8] = include_bytes!(' + literal(bundle) + ');\n'
                  'pub const MANIFEST: &str = include_str!(' + literal(manifest) + ');\n'
                  'pub const TARGET: &str = ' + literal(metadata['target']) + ';\n')
    return metadata
