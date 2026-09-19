import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import { ref, reactive, computed, watch, h, effectScope, nextTick } from 'vue'
import * as sql from '../src/utils/dataGrid.ts'

// Run the actual setup scripts with Vue reactivity and mocked desktop I/O.
// This is not a DOM/end-to-end test; no production database is contacted.
function setupComponent(file, start, exposed, overrides) {
  const source = readFileSync(new URL(file, import.meta.url), 'utf8').match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
  const script = stripTypeScriptTypes(source.slice(source.indexOf(start)))
  const disposers = []
  const messages = []
  const dialogs = []
  const bindings = {
    ref, computed, watch, h, ...sql,
    defineExpose: () => {},
    onBeforeUnmount: callback => disposers.push(callback),
    useI18n: () => ({ t: key => key }),
    useMessage: () => Object.fromEntries(['success', 'warning', 'error'].map(kind => [kind, text => messages.push({ kind, text })])),
    useDialog: () => ({ warning: options => dialogs.push(options) }),
    save: async () => null, open: async () => null,
    writeTextFile: async () => {}, readTextFile: async () => '',
    ...overrides
  }
  const scope = effectScope()
  const state = scope.run(() => new Function(...Object.keys(bindings), `${script}\nreturn {${exposed}}`)(...Object.values(bindings)))
  return { state, messages, dialogs, close() { disposers.forEach(fn => fn()); scope.stop() } }
}
const settle = async () => { await nextTick(); await new Promise(resolve => setImmediate(resolve)); await nextTick() }
const config = { id: 'a', name: 'A', db_type: 'postgresql', host: 'localhost', port: 5432 }
const columns = [{ name: 'tenant', is_pk: true, type_name: 'INT4' }, { name: 'id', is_pk: true, type_name: 'INT4' }]

function grid(props, invoke) {
  return setupComponent('../src/components/DataGrid.vue', 'const props =',
    'data, total, page, sortColumn, sortOrder, handleDelete, handleSorterChange, openEdit, showModal, loadData',
    { defineProps: () => props, invoke })
}

test('grid: switching databases reloads an identically named table and rejects the old response', async () => {
  const props = reactive({ config: { ...config }, table: 'items', database: 'first' })
  let resolveOld
  const calls = []
  const harness = grid(props, async (command, args) => {
    calls.push({ command, args })
    if (command === 'get_columns') return columns
    if (args.query.includes('COUNT(*)')) return [{ cx: 1 }]
    if (args.config.database === 'first') return new Promise(resolve => { resolveOld = resolve })
    return [{ tenant: 2, id: 2 }]
  })
  try {
    await settle()
    assert.equal(typeof resolveOld, 'function')
    props.database = 'second'
    await settle()
    assert.deepEqual(harness.state.data.value, [{ tenant: 2, id: 2 }])
    resolveOld([{ tenant: 1, id: 1 }])
    await settle()
    assert.deepEqual(harness.state.data.value, [{ tenant: 2, id: 2 }])
    assert.ok(calls.some(call => call.command === 'get_columns' && call.args.database === 'second'))
  } finally { harness.close() }
})

test('grid: an old delete confirmation cannot mutate the newly selected table', async () => {
  const props = reactive({ config: { ...config }, table: 'items', database: 'first' })
  const writes = []
  const harness = grid(props, async (command, args) => {
    if (command === 'get_columns') return columns
    if (args.query.startsWith('DELETE')) writes.push(args)
    return args.query.includes('COUNT(*)') ? [{ cx: 1 }] : [{ tenant: 7, id: 42 }]
  })
  try {
    await settle()
    harness.state.handleDelete({ tenant: 7, id: 42 })
    assert.equal(harness.dialogs.length, 1)
    props.database = 'second'
    await settle()
    await harness.dialogs[0].onPositiveClick()
    assert.equal(writes.length, 0)
    assert.ok(harness.messages.some(message => message.kind === 'warning'))
    harness.state.handleDelete({ tenant: 7, id: 42 })
    await harness.dialogs[1].onPositiveClick()
    assert.equal(writes.length, 1)
    assert.equal(writes[0].config.database, 'second')
    assert.equal(writes[0].query, 'DELETE FROM "items" WHERE "tenant" = 7 AND "id" = 42')
  } finally { harness.close() }
})

test('grid: switching tables resets sorting and closes an obsolete edit modal', async () => {
  const props = reactive({ config: { ...config }, table: 'items', database: 'first' })
  const harness = grid(props, async (command, args) => command === 'get_columns' ? columns : args.query.includes('COUNT(*)') ? [{ cx: 1 }] : [])
  try {
    await settle()
    harness.state.handleSorterChange({ columnKey: 'id', order: 'descend' })
    harness.state.openEdit({ tenant: 7, id: 42 })
    assert.equal(harness.state.showModal.value, true)
    props.table = 'other'
    await settle()
    assert.equal(harness.state.sortColumn.value, null)
    assert.equal(harness.state.sortOrder.value, false)
    assert.equal(harness.state.showModal.value, false)
  } finally { harness.close() }
})

test('manage: selection does not mutate the tree config, and route reuse reloads the connection', async () => {
  const route = reactive({ params: { id: 'a' } })
  const harness = setupComponent('../src/views/Manage.vue', 'const route =',
    'config, queryConfig, selectedTable, selectedDatabase, handleTableSelect, queryRef', {
      useRoute: () => route, useRouter: () => ({ push() {} }),
      invoke: async () => [{ ...config }, { ...config, id: 'b', name: 'B', database: 'fixed' }]
    })
  try {
    await settle()
    harness.state.handleTableSelect({ table: 'items', database: 'first' })
    assert.equal(harness.state.config.value.database, undefined)
    assert.equal(harness.state.queryConfig.value.database, 'first')
    harness.state.handleTableSelect({ table: 'items', database: 'second' })
    assert.equal(harness.state.config.value.database, undefined)
    assert.equal(harness.state.queryConfig.value.database, 'second')
    const queries = []
    harness.state.queryRef.value = { setQuery: query => queries.push(query) }
    harness.state.handleTableSelect({ table: 'order', database: 'second' })
    assert.equal(queries[0], 'SELECT * FROM "order" LIMIT 100;')
    route.params.id = 'b'
    await settle()
    assert.equal(harness.state.config.value.id, 'b')
    assert.equal(harness.state.selectedTable.value, '')
    assert.equal(harness.state.queryConfig.value.database, 'fixed')
  } finally { harness.close() }
})

test('Redis viewer reloads same key on database change and rejects old result', async () => {
  const props = reactive({ config: { ...config, db_type: 'redis' }, selectedKey: 'same', database: 'db0' })
  const pending = []
  const component = setupComponent('../src/components/RedisViewer.vue', 'const props =', 'keyInfo,error,loadKeyInfo', {
    defineProps: () => props,
    invoke: (_name, args) => new Promise(resolve => pending.push({ args, resolve }))
  })
  try {
    assert.equal(pending.length, 1)
    props.database = 'db1'
    await settle()
    pending[1].resolve({ key: 'same', value: 'new' }); await settle()
    pending[0].resolve({ key: 'same', value: 'old' }); await settle()
    assert.equal(component.state.keyInfo.value.value, 'new')
  } finally { component.close() }
})

test('query console ignores a result after changing the database', async () => {
  const props = reactive({ config: { ...config }, selectedDatabase: 'first' })
  let resolve
  const component = setupComponent('../src/components/QueryConsole.vue', 'const props =', 'query,results,runQuery', {
    defineProps: () => props, invoke: () => new Promise(done => { resolve = done })
  })
  try {
    component.state.query.value = 'SELECT 1'
    const pending = component.state.runQuery()
    props.selectedDatabase = 'second'; await settle()
    resolve([{ result: 'old' }]); await pending
    assert.deepEqual(component.state.results.value, [])
  } finally { component.close() }
})

test('AI cannot transmit a prompt without explicit consent', async () => {
  const calls = []
  const component = setupComponent('../src/components/QueryConsole.vue', 'const props =', 'aiPrompt,aiConsent,generateSQL', {
    defineProps: () => ({ config }), invoke: async (...args) => { calls.push(args); return [] }
  })
  try {
    component.state.aiPrompt.value = 'private prompt'
    await component.state.generateSQL()
    assert.equal(calls.length, 0)
  } finally { component.close() }
})

test('schema editor blocks stale deletion and combines rename/modify', async () => {
  const props = reactive({ config: { ...config }, table: 'same', database: 'first' })
  const calls = []
  const component = setupComponent('../src/components/TableStructure.vue', 'const props =', 'handleDrop,openEdit,formModel,handleSubmit', {
    defineProps: () => props, invoke: async (...args) => { calls.push(args); return [] }
  })
  try {
    await settle()
    component.state.handleDrop({ name: 'old', type_name: 'integer', is_pk: false })
    props.database = 'second'; await settle()
    await component.dialogs[0].onPositiveClick()
    assert.equal(calls.filter(([name]) => name === 'alter_table').length, 0)
    component.state.openEdit({ name: 'old', type_name: 'integer', is_pk: false })
    component.state.formModel.value.name = 'new'
    await component.state.handleSubmit()
    const writes = calls.filter(([name]) => name === 'alter_table')
    assert.equal(writes.length, 1)
    assert.equal(writes[0][1].operation.column_name, 'old')
    assert.equal(writes[0][1].operation.column_def.name, 'new')
  } finally { component.close() }
})
