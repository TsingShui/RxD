"""RxD source_id hashes length and first/last 16 KiB; provenance retains full SHA-256."""

import hashlib

SAMPLE_BYTES = 16 * 1024


def input_id(data):
    """Identity digest of a content buffer, matching `rxd_core::identity::input_id`."""
    hasher = hashlib.sha256()
    hasher.update(len(data).to_bytes(8, "little"))
    if len(data) <= 2 * SAMPLE_BYTES:
        hasher.update(data)
    else:
        hasher.update(data[:SAMPLE_BYTES])
        hasher.update(data[-SAMPLE_BYTES:])
    return hasher.hexdigest()


def input_identity(path):
    """Identity digest of a file's content."""
    return input_id(path.read_bytes())
