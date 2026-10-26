# PyInstaller corpus

`hello` is a one-file Mach-O arm64 executable built from a three-line script with
PyInstaller 6.22.3 and Python 3.9.6. It contains a CArchive with a PYZ archive,
CPython framework files, and extension modules. Its build command was:

```console
pyinstaller --onefile --clean --noconfirm --distpath dist --workpath build --specpath . hello.py
```

The archive and the expected receipt are pinned in `expectations.json`. The CArchive
code, PYZ modules and reconstructed `.pyc` files are verified byte-for-byte against
that pinned output. Runtime code is never executed. A PyInstaller one-dir directory is
handled by locating its launcher executable; loose `_internal` dependencies remain in
the input tree.

The executable embeds CPython 3.9.6, distributed under the Python Software Foundation
License Version 2; see <https://docs.python.org/3.9/license.html>. The test script is
original and dedicated to the repository's license.
