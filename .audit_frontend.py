from pathlib import Path
import json
from importlib.machinery import SourceFileLoader

def replace(text, old, new, count=1):
    assert text.count(old) == count, (old[:100], text.count(old), count)
    return text.replace(old, new)
def section(text, start, end, replacement):
    a = text.index(start); b = text.index(end, a)
    return text[:a] + replacement + text[b:]

path = Path('src/components/TableStructure.vue'); s = path.read_text()
s = replace(s, "import { ref, computed, watch, h }", "import { ref, computed, watch, h, onBeforeUnmount }")
s = section(s, 'async function loadColumns()', '</script>', r'''let generation = 0
let columnsRequest = 0
let indexesRequest = 0
const submitting = ref(false)
type Target = { config: ConnectionConfig, table: string, generation: number }
let modalTarget: Target | null = null
let indexTarget: Target | null = null
function snapshot(): Target {
    return { config: { ...props.config, database: props.database ?? props.config.database }, table: props.table, generation }
}
function current(target: Target) { return target.generation === generation }
async function loadColumns() {
    const target = snapshot(), request = ++columnsRequest
    loading.value = true
    try {
        const result = await invoke<ColumnDef[]>('get_columns', { config: target.config, table: target.table, database: target.config.database })
        if (current(target) && request === columnsRequest) columns.value = result
    } catch (e) { if (current(target) && request === columnsRequest) message.error(String(e)) }
    finally { if (current(target) && request === columnsRequest) loading.value = false }
}
async function loadIndexes() {
    const target = snapshot(), request = ++indexesRequest
    loadingIndexes.value = true
    try {
        const result = await invoke<IndexDef[]>('get_indexes', { config: target.config, table: target.table })
        if (current(target) && request === indexesRequest) indexes.value = result
    } catch (e) { if (current(target) && request === indexesRequest) message.error(String(e)) }
    finally { if (current(target) && request === indexesRequest) loadingIndexes.value = false }
}
watch(() => [props.table, props.database, JSON.stringify(props.config)], () => {
    generation++
    columns.value = []; indexes.value = []
    showModal.value = false; showIndexModal.value = false
    modalTarget = null; indexTarget = null
    if (props.table) { void loadColumns(); void loadIndexes() }
}, { immediate: true })
onBeforeUnmount(() => { generation++ })
function openAdd() {
    modalTarget = snapshot(); modalMode.value = 'add'
    formModel.value = { name: '', type_name: 'VARCHAR(255)', is_pk: false, is_nullable: true, default_value: '', comment: '' }
    showModal.value = true
}
function openEdit(row: ColumnDef) {
    modalTarget = snapshot(); modalMode.value = 'edit'; originalName.value = row.name
    formModel.value = { name: row.name, type_name: row.type_name, is_pk: row.is_pk,
        is_nullable: row.is_nullable !== false, default_value: row.default_value ?? '', comment: row.comment ?? '' }
    showModal.value = true
}
function openAddIndex() {
    indexTarget = snapshot(); indexForm.value = { name: '', columns: [], is_unique: false }; showIndexModal.value = true
}
async function perform(target: Target | null, operation: Record<string, unknown>) {
    if (!target || !current(target)) { message.warning('Selection changed; schema operation cancelled.'); return false }
    if (submitting.value) return false
    submitting.value = true
    try {
        await invoke('alter_table', { config: target.config, table: target.table, operation })
        message.success(t('common.success'))
        if (current(target)) { void loadColumns(); void loadIndexes() }
        return true
    } catch (e) { message.error(String(e)); return false }
    finally { submitting.value = false }
}
async function handleIndexSubmit() {
    if (!indexForm.value.name.trim() || !indexForm.value.columns.length) { message.warning('Index name and columns are required.'); return }
    const target = indexTarget
    if (await perform(target, { op_type: 'add_index', index_def: { ...indexForm.value, is_pk: false, comment: null } }) && target && current(target)) showIndexModal.value = false
}
function handleDropIndex(row: IndexDef) {
    if (row.is_pk) return
    const target = snapshot(), name = row.name
    dialog.warning({ title: t('common.delete'), content: `Drop index ${name}?`, positiveText: t('common.delete'), negativeText: t('common.cancel'),
        onPositiveClick: () => perform(target, { op_type: 'drop_index', index_name: name }) })
}
function handleDrop(row: ColumnDef) {
    if (row.is_pk) return
    const target = snapshot(), name = row.name
    dialog.warning({ title: t('common.delete'), content: t('structure.drop_confirm', { name }), positiveText: t('common.delete'), negativeText: t('common.cancel'),
        onPositiveClick: () => perform(target, { op_type: 'drop', column_name: name }) })
}
async function handleSubmit() {
    if (!formModel.value.name.trim()) { message.warning('Column name is required.'); return }
    const target = modalTarget
    const definition = { ...formModel.value, default_value: formModel.value.default_value || null, comment: formModel.value.comment || null }
    // Rename and definition changes are one backend operation, not two independent RPCs.
    const success = await perform(target, { op_type: modalMode.value === 'add' ? 'add' : 'modify',
        column_name: modalMode.value === 'add' ? definition.name : originalName.value, column_def: definition })
    if (success && target && current(target)) showModal.value = false
}
''')
s = s.replace('@click="handleSubmit"', ':loading="submitting" @click="handleSubmit"').replace('@click="handleIndexSubmit"', ':loading="submitting" @click="handleIndexSubmit"')
path.write_text(s)

path = Path('src/components/RedisViewer.vue'); s = path.read_text()
s = replace(s, "import { ref, watch }", "import { ref, watch, onBeforeUnmount }")
s = section(s, 'async function loadKeyInfo()', 'function getTypeColor', r'''let requestId = 0
async function loadKeyInfo() {
  const request = ++requestId
  keyInfo.value = null; error.value = ''
  if (!props.selectedKey) { loading.value = false; return }
  const config = { ...props.config }, key = props.selectedKey, database = props.database
  loading.value = true
  try {
    const info = await invoke<RedisKeyInfo>('get_redis_key_value', { config, key, database })
    if (request === requestId) keyInfo.value = info
  } catch (e) { if (request === requestId) error.value = String(e) }
  finally { if (request === requestId) loading.value = false }
}
watch(() => [props.selectedKey, props.database, JSON.stringify(props.config)], loadKeyInfo, { immediate: true })
onBeforeUnmount(() => { requestId++ })

''')
path.write_text(s)

path = Path('src/views/Manage.vue'); s = path.read_text()
s = replace(s, 'JSON.stringify(config, null, 2)', "JSON.stringify({ ...config, password: config.password ? '••••••' : undefined }, null, 2)")
path.write_text(s)

path = Path('src/components/DataGrid.vue'); s = path.read_text()
s = replace(s, 'async function triggerImport() {\n    if (!schemaReady) return', 'async function triggerImport() {\n    if (!schemaReady || loading.value) return')
s = replace(s, 'const queries = rows.map(row => {', 'rows.forEach(row => {')
s = replace(s, '''        for (const query of queries) {
            if (!isCurrent(target)) throw new Error('Selection changed; remaining import cancelled')
            await invoke('execute_query', { config: target.config, query })
            successCount++
        }''', '''        if (!isCurrent(target)) throw new Error('Selection changed; import cancelled')
        successCount = await invoke<number>('import_table_rows', { config: target.config, table: target.table, rows })''')
s = replace(s, '        // Row-by-row imports are not atomic; report the committed prefix honestly.', '        // The backend reports rollback or an uncertain commit; never imply that retry is safe.')
s = replace(s, "message.error(`${t('manage.import_failed')}: ${e.toString()} (${successCount} rows imported)`)", "message.error(`${t('manage.import_failed')}: ${e.toString()}`)")
s = replace(s, 'async function handleExport(key: string) {', '''function hasBinaryColumns() { return tableMetadata.value.some(c => /binary|blob|bytea|bit\\b/i.test(c.type_name)) }
function guardBinaryValues() {
    if (!hasBinaryColumns()) return true
    message.warning('Binary columns require a typed binary workflow. Use the SQL console or database-level SQL export; text import/export/editing is disabled to prevent corruption.')
    return false
}
async function handleExport(key: string) {\n    if (!guardBinaryValues()) return''')
s = replace(s, 'function openEdit(row: any) {', 'function openEdit(row: any) {\n    if (!guardBinaryValues()) return')
s = replace(s, 'async function triggerImport() {', 'async function triggerImport() {\n    if (!guardBinaryValues()) return')
# Keep decimal and bigint values as text so the numeric widget cannot round them.
s = replace(s, '''                 <NInputNumber v-else-if="['INT', 'FLOAT', 'DOUBLE', 'DECIMAL', 'NUMERIC', 'REAL'].some(t => col.type_name.toUpperCase().includes(t))"''', '''                 <NInput v-else-if="/BIGINT|INT8|DECIMAL|NUMERIC/i.test(col.type_name) || typeof formData[col.name] === 'string'" v-model:value="formData[col.name]" :disabled="modalMode === 'edit' && col.is_pk" />
                 <NInputNumber v-else-if="['INT', 'FLOAT', 'DOUBLE', 'REAL'].some(t => col.type_name.toUpperCase().includes(t))"''')
path.write_text(s)

path = Path('src/components/QueryConsole.vue'); s = path.read_text()
s = replace(s, 'ref, watch, computed', 'ref, watch, computed, onBeforeUnmount')
s = replace(s, 'NIcon, useMessage, NAlert, NModal, NFormItem', 'NIcon, useMessage, NAlert, NModal, NFormItem, NCheckbox')
s = replace(s, 'const aiLoading = ref(false)', '''const aiLoading = ref(false)
const aiConsent = ref(false)
let generation = 0
let queryRequest = 0
let aiRequest = 0
watch(() => [JSON.stringify(props.config), props.selectedTable, props.selectedDatabase], () => {
  generation++; queryRequest++; aiRequest++
  results.value = []; error.value = ''; lastQuery.value = ''
  loading.value = false; aiLoading.value = false; showAIModal.value = false; aiConsent.value = false
})
onBeforeUnmount(() => { generation++; queryRequest++; aiRequest++ })''')
s = section(s, 'async function runQuery()', 'function openAIModal()', r'''async function runQuery() {
  if (!query.value.trim() || loading.value) return
  const request = ++queryRequest, target = generation
  const sql = query.value, config = { ...props.config, database: props.selectedDatabase ?? props.config.database }
  loading.value = true; error.value = ''; results.value = []
  const start = performance.now()
  try {
    const data = await invoke<any[]>('execute_query', { config, query: sql })
    if (target !== generation || request !== queryRequest) return
    results.value = data.map((item, index) => ({ ...item, __id: index }))
    lastQuery.value = sql; executionTime.value = Math.round(performance.now() - start)
    const failures = data.filter(row => row.error).map(row => String(row.error))
    if (failures.length) error.value = failures.join('\n')
    else message.success(t('manage.query_success', { time: executionTime.value, rows: data.length }))
  } catch (e) { if (target === generation && request === queryRequest) error.value = String(e) }
  finally { if (target === generation && request === queryRequest) loading.value = false }
}

''')
s = replace(s, "  aiPrompt.value = ''\n  showAIModal.value = true", "  aiPrompt.value = ''; aiConsent.value = false\n  showAIModal.value = true")
s = section(s, 'async function generateSQL()', '// Expose run function', r'''async function generateSQL() {
  if (!aiPrompt.value.trim()) { message.warning(t('ai.enter_prompt')); return }
  if (!aiConsent.value) { message.warning(t('ai.consent_required')); return }
  if (aiLoading.value) return
  const request = ++aiRequest, target = generation
  const config = { ...props.config }, table = props.selectedTable, database = props.selectedDatabase, prompt = aiPrompt.value
  aiLoading.value = true
  try {
    let tableSchemas = '(No table selected)'
    if (table) {
      const columns = await invoke<ColumnDef[]>('get_columns', { config, table, database })
      tableSchemas = `Table: ${table}\n` + columns.map(c => `${c.name} (${c.type_name})`).join('\n')
    }
    if (target !== generation || request !== aiRequest) return
    const sql = await invoke<string>('generate_sql_from_text', { dbType: config.db_type, tableSchemas, userRequest: prompt })
    if (target !== generation || request !== aiRequest) return
    query.value = sql; showAIModal.value = false
    message.success(t('ai.sql_generated'))
  } catch (e) { if (target === generation && request === aiRequest) message.error(String(e)) }
  finally { if (target === generation && request === aiRequest) aiLoading.value = false }
}

''')
s = replace(s, '      <NFormItem :label="t(\'ai.describe_query\')">', '''      <NAlert type="warning" style="margin-bottom: 12px">{{ t('ai.privacy_notice') }}</NAlert>
      <NCheckbox v-model:checked="aiConsent" style="margin-bottom: 12px">{{ t('ai.consent_required') }}</NCheckbox>
      <NFormItem :label="t('ai.describe_query')">''')
s = replace(s, '@click="generateSQL" :loading="aiLoading"', '@click="generateSQL" :loading="aiLoading" :disabled="!aiConsent"')
path.write_text(s)
for locale in ['en', 'zh-CN']:
    path = Path(f'src/i18n/locales/{locale}.json'); content = json.loads(path.read_text())
    content['ai']['privacy_notice'] = 'Your prompt and selected table schema will be sent to the configured AI endpoint. Do not include credentials or personal data. Review generated queries before running them.' if locale == 'en' else '你的需求和当前表结构将发送到已配置的 AI 服务地址。请勿填写密码或个人敏感数据，生成的语句需要审查后再执行。'
    content['ai']['consent_required'] = 'I agree to send this request and schema to the configured AI service.' if locale == 'en' else '我同意将本次需求和表结构发送到已配置的 AI 服务。'
    path.write_text(json.dumps(content, ensure_ascii=False, indent=2) + '\n')

path = Path('README.md'); s = path.read_text()
s = s.replace('Your data never leaves your device.', 'Database operations are local to this application. AI generation sends your prompt and selected schema to the configured AI provider only after explicit confirmation.')
s = s.replace('数据永远不离开你的设备。', '数据库操作由本机发起；AI 生成功能经明确确认后，会将需求和选定表结构发送到配置的 AI 服务。')
s = s.replace('Node.js (v16+)', 'Node.js (v22.13+)').replace('Rust (Stable)', 'Rust 1.89+ (Stable)')
s += '''\n## Safety and supported limits / 安全与支持边界\n\n- Connection and AI configuration JSON is replaced atomically, locked against concurrent writers, and owner-only on Unix. This is **not encryption at rest**; Windows access relies on the user-profile directory ACL. Corrupt JSON is reported, never silently reset.\n- Row-file import uses a database transaction (MySQL requires InnoDB); arbitrary SQL-script import is not promised to be transactional because scripts may contain DDL or explicit commits. A network failure during COMMIT can leave the outcome unknown: verify before retrying.\n- Each Redis operation and raw SQL request owns its session. A manual SELECT, MULTI, USE, SET or transaction does not carry over to later requests. Put related commands in one request. Redis key browsing is bounded at 10000 keys; oversized values and unsupported SQL types produce an explicit error instead of truncated backup data.\n- The built-in database SQL exporter is a **limited logical table exporter**, not a replacement for pg_dump/mysqldump. It rejects detected unsupported objects, uses a consistent snapshot for supported transactional tables, and replaces the selected file only on success. Use native database backup tools for full schema, permissions, extension objects, custom sequence options and disaster recovery. Always test restoration into a disposable database.\n- Binary fields cannot be losslessly represented by this grid's generic CSV/JSON import/export or text editor, so those operations are blocked for binary tables. Database-level SQL export writes actual binary literals. Bigint and exact decimals are transported as text to avoid JavaScript rounding.\n- AI responses are untrusted suggestions and are never automatically executed. HTTPS is required except for explicitly configured loopback services.\n\n配置文件并非加密存储；本轮改进不等同于“零风险”认证。整库 SQL 导出仍有明确支持范围，生产备份应使用数据库原生工具，并在独立测试库验证恢复。跨平台单元测试和一次性数据库集成测试不替代真实桌面端到端及大规模压力测试。\n'''
path.write_text(s)
print('UI target guards, atomic import integration and privacy disclosures applied.')
