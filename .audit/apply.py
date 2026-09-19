from pathlib import Path
import hashlib, json, lzma
payload = b''.join(Path(f'.audit/part{i}').read_bytes() for i in range(4))
assert hashlib.sha256(payload).hexdigest() == '676a946a6a2e8ab1a15f2be722c37877545adf430e6d857b96596fe1b0fe3e72'
prepared = {}
for entry in json.loads(lzma.decompress(payload)):
    path = Path(entry['path'])
    assert not path.is_absolute() and '..' not in path.parts and path not in prepared
    old = path.read_bytes() if path.exists() else None
    assert (hashlib.sha256(old).hexdigest() if old is not None else None) == entry['old'], str(path)
    lines = old.decode('utf-8').splitlines(keepends=True) if old is not None else []
    for start, end, text in reversed(entry['edits']):
        lines[start:end] = [text]
    content = ''.join(lines).encode('utf-8')
    assert hashlib.sha256(content).hexdigest() == entry['new'], str(path)
    prepared[path] = content
for path, content in prepared.items():
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
print(f'Applied {len(prepared)} source changes with before/after SHA256 validation.')
