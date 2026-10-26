# Pinned ZIP-family production corpus

These published archives are parsed as data, never installed or executed.
`expectations.json` records independent Python `zipfile` metadata, selected
payload sizes and SHA-256 hashes; the unpack E2E extracts the whole archive and
checks those selected files.

| Artifact | Source | Bytes | SHA-256 | License |
| --- | --- | ---: | --- | --- |
| AndroidX Core 1.15.0 AAR | [Google Maven](https://dl.google.com/dl/android/maven2/androidx/core/core/1.15.0/core-1.15.0.aar) | 1336135 | 432b85a1974076e14b487ece4a28c59a84f1b9efc3fc8be72cd7f05d32055e51 | Apache-2.0, embedded `META-INF/androidx/core/core/LICENSE.txt` |
| Red Hat YAML 2.0.0 VSIX, build 400 | [Published release](https://github.com/redhat-developer/vscode-yaml/releases/tag/2.0.0) | 1104200 | 4bba62ccda962b720dcb0829e85cf4109c94e86653dcd39fd7500c3687ad8c60 | MIT, embedded `extension/LICENSE.txt`; dependency notices in `extension/thirdpartynotices.txt` |

The VSIX hash also matches the release's published
`vscode-yaml-2.0.0-400.vsix.sha256`. URLs and selected payload hashes are retained
in `expectations.json`. Existing Gson/Rhino JAR inputs retain their provenance in
[the JVM corpus](../jvm/README.md).

Re-download from the recorded URLs and verify full archive SHA-256 before use.
`selected[].at` is retained only as independent central-directory metadata; the
current CLI has no per-entry selection. Verify payload hashes without RxD:

```sh
python3 - <<'PY'
import hashlib, json, zipfile
from pathlib import Path
root = Path('tests/corpus/zip')
for expected in json.loads((root / 'expectations.json').read_text()):
    path = root / expected['file']
    assert hashlib.sha256(path.read_bytes()).hexdigest() == expected['sha256']
    with zipfile.ZipFile(path) as archive:
        for selected in expected['selected']:
            data = archive.read(selected['path'])
            assert len(data) == selected['bytes']
            assert hashlib.sha256(data).hexdigest() == selected['sha256']
PY
```

Handcrafted unpack E2E inputs separately cover duplicate and unsafe paths,
symlinks, Stored/Deflate decoding, UTF-8, destination collisions and cleanup.
