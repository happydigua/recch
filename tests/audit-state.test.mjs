import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import { ref, reactive, computed, watch, h, effectScope, nextTick } from 'vue'
import * as sql from '../src/utils/dataGrid.ts'

// Exercise the real setup scripts with Vue reactivity, not copied business logic.
// Desktop IPC, dialogs and files are mocked; live DB checks are separate Rust tests.
function component(file, exposed, props, overrides = {}) {
  const source = readFileSync(new URL(`../src/components/${file}.vue`, import.meta.url), 'utf8').match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
  const script = stripTypeScriptTypes(source.slice(source.indexOf('const props =')))
  const messages = [], dialogs = [], disposers = []
  const bindings = {
    ref, computed, watch, h, ...sql,
    defineProps: () => props, defineExpose: () => {},
    onBeforeUnmount: callback => disposers.push(callback),
    useI18n: () => ({ t: key => key }),
    useMessage: () => Object.fromEntries(['success', 'warning', 'error'].map(kind => [kind, text => messages.push({kind, text})])),
    useDialog: () => ({ warning: options => dialogs.push(options) }),
    open: async () => null, save: async () => null, readTextFile: async () => '', writeTextFile: async () => {},
    ...overrides
  }
  const scope = effectScope()
  const state = scope.run(() => new Function(...Object.keys(bindings), `${script}\nreturn {${exposed}}`)(...Object.values(bindings)))
  return {state, messages, dialogs, close() { disposers.forEach(fn => fn()); scope.stop() } }
}
const settle = async () => { await nextTick(); await new Promise(resolve => setImmediate(resolve)); await nextTick() }
const config = { id: 'test', name: 'Test', db_type: 'postgresql', host: 'localhost', port: 5432, database: 'first' }
const column = { name: 'title', type_name: 'text', is_pk: false, is_nullable: true }

test('structure: old drop confirmation never operates on another database', async () => {
  const props = reactive({config: {...config}, table: 'items', database: 'first'})
  const writes = []
  const harness = component('TableStructure', 'handleDrop, columns', props, {invoke: async (cmd, args) => {
    if (cmd === 'alter_table') writes.push(args)
    return cmd === 'get_columns' ? [column] : []
  }})
  try {
    await settle(); harness.state.handleDrop(column)
    props.database = 'second'; await settle()
    await harness.dialogs[0].onPositiveClick()
    assert.equal(writes.length, 0)
    assert.ok(harness.messages.some(m => m.kind === 'warning'))
    harness.state.handleDrop(column); await harness.dialogs[1].onPositiveClick()
    assert.equal(writes[0].config.database, 'second')
  } finally { harness.close() }
})

test('structure: stale metadata is ignored and old edit form is closed', async () => {
  const props = reactive({config: {...config}, table: 'items', database: 'first'})
  let resolveOld
  const harness = component('TableStructure', 'columns, openEdit, showModal', props, {invoke: async (cmd, args) => {
    if (cmd !== 'get_columns') return []
    if (args.config.database === 'first') return new Promise(resolve => { resolveOld = resolve })
    return [{...column, name: 'new_column'}]
  }})
  try {
    await settle(); harness.state.openEdit(column); assert.equal(harness.state.showModal.value, true)
    props.database = 'second'; await settle()
    resolveOld([column]); await settle()
    assert.equal(harness.state.columns.value[0].name, 'new_column')
    assert.equal(harness.state.showModal.value, false)
  } finally { harness.close() }
})

test('structure: rename and definition change are submitted as one operation', async () => {
  const props = reactive({config: {...config}, table: 'items', database: 'first'})
  const writes = []
  const harness = component('TableStructure', 'openEdit, formModel, handleSubmit', props, {invoke: async (cmd, args) => {
    if (cmd === 'alter_table') writes.push(args)
    return cmd === 'get_columns' ? [column] : []
  }})
  try {
    await settle(); harness.state.openEdit(column); harness.state.formModel.value.name = 'renamed'
    await harness.state.handleSubmit()
    assert.equal(writes.length, 1); assert.equal(writes[0].operation.op_type, 'modify')
    assert.equal(writes[0].operation.column_name, 'title'); assert.equal(writes[0].operation.column_def.name, 'renamed')
  } finally { harness.close() }
})

test('redis viewer: same key in another database reloads and rejects stale results', async () => {
  const props = reactive({config: {...config, db_type: 'redis'}, selectedKey: 'key', database: 'first'})
  let resolveOld
  const harness = component('RedisViewer', 'keyInfo, error', props, {invoke: async (_cmd, args) => {
    if (args.database === 'first') return new Promise(resolve => { resolveOld = resolve })
    return { key: 'key', key_type: 'string', ttl: -1, value: 'second' }
  }})
  try {
    await settle(); props.database = 'second'; await settle()
    resolveOld({key: 'key', value: 'first'}); await settle()
    assert.equal(harness.state.keyInfo.value.value, 'second')
  } finally { harness.close() }
})

test('query console: running query snapshots text and discards results after switching database', async () => {
  const props = reactive({config: {...config}, selectedDatabase: 'first'})
  let resolveQuery
  const harness = component('QueryConsole', 'query, runQuery, results, lastQuery', props, {invoke: async () => new Promise(resolve => { resolveQuery = resolve })})
  try {
    harness.state.query.value = 'SELECT 1'; const run = harness.state.runQuery()
    props.selectedDatabase = 'second'; await settle()
    resolveQuery([{result: 'old'}]); await run
    assert.deepEqual(harness.state.results.value, []); assert.equal(harness.state.lastQuery.value, '')
  } finally { harness.close() }
})

test('AI generation requires consent and never sends stale table context', async () => {
  const props = reactive({config: {...config}, selectedTable: 'items', selectedDatabase: 'first'})
  let resolveColumns
  const calls = []
  const harness = component('QueryConsole', 'aiPrompt, aiConsent, generateSQL', props, {invoke: async (cmd) => {
    calls.push(cmd)
    if (cmd === 'get_columns') return new Promise(resolve => { resolveColumns = resolve })
    return 'SELECT 1'
  }})
  try {
    harness.state.aiPrompt.value = 'list items'; await harness.state.generateSQL(); assert.equal(calls.length, 0)
    harness.state.aiConsent.value = true; const generate = harness.state.generateSQL()
    props.selectedDatabase = 'second'; await settle(); resolveColumns([column]); await generate
    assert.deepEqual(calls, ['get_columns'])
  } finally { harness.close() }
})

test('grid imports rows with one transactional IPC call instead of sequential writes', async () => {
  const props = reactive({config: {...config}, table: 'items', database: 'first'})
  const imports = [], sqlWrites = []
  const harness = component('DataGrid', 'triggerImport', props, {
    open: async () => '/fixture/input.json', readTextFile: async () => '[{"title":""},{"title":null}]',
    invoke: async (cmd, args) => {
      if (cmd === 'get_columns') return [column]
      if (cmd === 'import_table_rows') { imports.push(args); return 2 }
      if (args.query.startsWith('INSERT')) sqlWrites.push(args)
      return args.query.includes('COUNT(*)') ? [{cx: 0}] : []
    }
  })
  try {
    await settle(); await harness.state.triggerImport()
    assert.equal(imports.length, 1); assert.equal(sqlWrites.length, 0)
    assert.deepEqual(imports[0].rows, [{title: ''}, {title: null}])
  } finally { harness.close() }
})

test('grid binary text import is blocked rather than silently storing preview text', async () => {
  const props = reactive({config: {...config}, table: 'items', database: 'first'})
  let opened = false
  const harness = component('DataGrid', 'triggerImport', props, {
    open: async () => { opened = true; return '/fixture/input.json' },
    invoke: async (cmd, args) => cmd === 'get_columns' ? [{...column, type_name: 'bytea'}] : args.query.includes('COUNT(*)') ? [{cx: 0}] : []
  })
  try {
    await settle(); await harness.state.triggerImport(); assert.equal(opened, false)
    assert.ok(harness.messages.some(m => m.kind === 'warning'))
  } finally { harness.close() }
})


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
