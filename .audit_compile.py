from pathlib import Path
import re

p = Path('src-tauri/src/table_import.rs')
s = p.read_text()
old = 'sqlx::query_scalar("SELECT ENGINE FROM information_schema.TABLES'
assert s.count(old) == 1
s = s.replace(old, 'sqlx::query_scalar::<_, Option<String>>("SELECT ENGINE FROM information_schema.TABLES')
p.write_text(s)
p = Path('src-tauri/src/lib.rs')
s = p.read_text()
old = 'v.get(1).and_then(|s| s.parse().ok())'
assert s.count(old) == 1
s = s.replace(old, 'v.get(1).and_then(|s| s.parse::<usize>().ok())')
# Executor::execute returns a boxed Send future with explicit lifetimes. Avoid
# RawSql::execute's generic async wrapper at Tauri's Send command boundary.
pattern = r'raw_sql\(("[^"\n]*"|&script)\)\s*\.execute\((&mut \*(?:source|connection))\)'
def executor(match):
    query = 'script.as_str()' if match[1] == '&script' else match[1]
    return 'sqlx::Executor::execute(' + match[2] + ', ' + query + ')'
s, count = re.subn(pattern, executor, s)
assert count == 4, ('SQL batch executors', count)
s = s.replace('use sqlx::raw_sql;\n', '')
p.write_text(s)
p = Path('src/components/DataGrid.vue')
s = p.read_text()
for name, args in [('handleDelete', 'row: any'), ('openCreate', '')]:
    pattern = r'((?:async )?function ' + name + r'\(' + re.escape(args) + r'\) \{)'
    s, count = re.subn(pattern, r'\1\n    if (!guardBinaryValues()) return', s)
    assert count == 1, (name, count)
p.write_text(s)
p = Path('tests/audit-state.test.mjs')
s = p.read_text() + r'''

test('grid refuses binary-primary-key delete instead of matching a textual hex preview', async () => {
  const props = reactive({config: {...config}, table: 'items', database: 'first'})
  const writes = []
  const harness = component('DataGrid', 'handleDelete, openCreate, showModal', props, {
    invoke: async (cmd, args) => {
      if (cmd === 'get_columns') return [{...column, name: 'id', type_name: 'bytea', is_pk: true}]
      if (args.query.startsWith('DELETE')) writes.push(args)
      return args.query.includes('COUNT(*)') ? [{cx: 0}] : []
    }
  })
  try {
    await settle(); await harness.state.handleDelete({id: '0xFF'})
    assert.equal(harness.dialogs.length, 0); assert.equal(writes.length, 0)
    harness.state.openCreate(); assert.equal(harness.state.showModal.value, false)
  } finally { harness.close() }
})
'''
p.write_text(s)
print('SQL executor lifetimes, Rust scalar types and binary mutation guards corrected.')
