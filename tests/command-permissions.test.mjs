import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'

const backend = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8')
const permissions = readFileSync(new URL('../src-tauri/permissions/db.toml', import.meta.url), 'utf8')
const registered = new Set(backend.match(/generate_handler!\[([\s\S]*?)\]/)[1].split(',').map(s => s.trim()).filter(Boolean))
const allowed = new Set([...permissions.match(/commands\.allow\s*=\s*\[([\s\S]*?)\]/)[1].matchAll(/"([a-z_]+)"/g)].map(m => m[1]))

test('desktop IPC allowlist matches registered commands, including transactional import', () => {
  assert.deepEqual([...allowed].sort(), [...registered].sort())
  assert.ok(allowed.has('import_table_rows'))
})

test('literal frontend database invocations have both a backend handler and permission', () => {
  const root = new URL('../src/', import.meta.url)
  for (const path of readdirSync(root, {recursive: true}).filter(p => /\.(vue|ts)$/.test(p))) {
    const text = readFileSync(new URL(path, root), 'utf8')
    for (const match of text.matchAll(/invoke(?:<[^>]*>)?\(\s*'([a-z_]+)'/g)) {
      assert.ok(registered.has(match[1]), `${path}: unregistered ${match[1]}`)
      assert.ok(allowed.has(match[1]), `${path}: unauthorized ${match[1]}`)
    }
  }
})
