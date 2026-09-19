import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { stripTypeScriptTypes } from 'node:module'
import { ref, watch, computed, reactive, effectScope } from 'vue'

// Run the production setup script with real Vue state and mocked desktop IPC.
function consoleHarness(dbType, invoke) {
  const source = readFileSync(new URL('../src/components/QueryConsole.vue', import.meta.url), 'utf8')
    .match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1]
  const script = stripTypeScriptTypes(source.slice(source.indexOf('const props =')))
  const props = reactive({ config: { id: 'fixture', name: 'Fixture', db_type: dbType, host: 'localhost', port: 5432 } })
  const messages = [], disposers = []
  const bindings = {
    ref, watch, computed, invoke,
    defineProps: () => props, defineExpose: () => {},
    onBeforeUnmount: callback => disposers.push(callback),
    useI18n: () => ({ t: key => key }),
    useMessage: () => Object.fromEntries(['success', 'warning', 'error'].map(kind => [kind, text => messages.push({kind, text})]))
  }
  const scope = effectScope()
  const state = scope.run(() => new Function(...Object.keys(bindings), `${script}\nreturn {query, runQuery, results, columns, error, loading}`)(...Object.values(bindings)))
  return { state, messages, close() { disposers.forEach(fn => fn()); scope.stop() } }
}

for (const dbType of ['mysql', 'postgresql']) {
  test(`${dbType}: SQL column names never collide with UI identity or error status`, async () => {
    const original = JSON.parse('{"__id":"database-id","rowId":"database-row","values":{"value":1},"error":"stored error text","__proto__":"ordinary column"}')
    const harness = consoleHarness(dbType, async () => [original])
    try {
      harness.state.query.value = 'SELECT * FROM fixture'
      await harness.state.runQuery()
      assert.equal(harness.state.error.value, '')
      assert.equal(harness.messages.filter(m => m.kind === 'success').length, 1)
      const row = harness.state.results.value[0]
      assert.equal(row.rowId, 0)
      assert.deepEqual(row.values, original)
      assert.deepEqual(harness.state.columns.value.map(c => c.key), Object.keys(original))
      assert.equal(harness.state.columns.value.find(c => c.key === '__id').render(row), 'database-id')
      assert.equal(harness.state.columns.value.find(c => c.key === 'error').render(row), 'stored error text')
      assert.equal(harness.state.columns.value.find(c => c.key === 'values').render(row), '{"value":1}')
      assert.equal(original.__id, 'database-id')
    } finally { harness.close() }
  })
}

test('Redis command errors still display as failures instead of successful queries', async () => {
  const harness = consoleHarness('redis', async () => [{error: 'WRONGTYPE fixture'}])
  try {
    harness.state.query.value = 'GET fixture'
    await harness.state.runQuery()
    assert.equal(harness.state.error.value, 'WRONGTYPE fixture')
    assert.equal(harness.messages.filter(m => m.kind === 'success').length, 0)
    assert.equal(harness.state.loading.value, false)
  } finally { harness.close() }
})

test('SQL IPC rejection is still reported as a query failure', async () => {
  const harness = consoleHarness('postgresql', async () => { throw new Error('Fixture connection failed') })
  try {
    harness.state.query.value = 'SELECT 1'
    await harness.state.runQuery()
    assert.match(harness.state.error.value, /Fixture connection failed/)
    assert.equal(harness.messages.filter(m => m.kind === 'success').length, 0)
    assert.equal(harness.state.loading.value, false)
  } finally { harness.close() }
})
