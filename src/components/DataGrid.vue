<script setup lang="ts">
import { ref, watch, computed, h, onBeforeUnmount } from 'vue'
import {
  NDataTable, NButton, NSpace, NIcon, NPagination, useMessage, useDialog,
  NModal, NForm, NFormItem, NInput, NInputNumber, NCheckbox, NSelect,
  NDropdown, NInputGroup, NEllipsis
} from 'naive-ui'
import {
  AddOutline, RefreshOutline, TrashOutline, CreateOutline,
  SearchOutline, DownloadOutline, CloudUploadOutline
} from '@vicons/ionicons5'
import { invoke } from '../utils/tauri'
import { save, open } from '@tauri-apps/plugin-dialog'
import { writeTextFile, readTextFile } from '@tauri-apps/plugin-fs'
import { useI18n } from 'vue-i18n'
import type { ConnectionConfig } from '../types'
import type { DataTableColumns } from 'naive-ui'
import {
  sqlDialect, quoteIdentifier, primaryKeyWhere, insertQuery, updateQuery,
  deleteQuery, searchWhere, parseCSV
} from '../utils/dataGrid'

const props = defineProps<{
  config: ConnectionConfig
  table: string
  database?: string
}>()

const message = useMessage()
const dialog = useDialog()
const { t } = useI18n()
const loading = ref(false)
const tableMetadata = ref<any[]>([])
const data = ref<any[]>([])
const total = ref(0)
const page = ref(1)
const pageSize = ref(100)
const sortColumn = ref<string | null>(null)
const sortOrder = ref<'ascend' | 'descend' | false>(false)
const pageSizeOptions = [
    { label: '20 行', value: 20 },
    { label: '50 行', value: 50 },
    { label: '100 行', value: 100 },
    { label: '500 行', value: 500 },
    { label: '1000 行', value: 1000 }
]

const searchKeyword = ref('')
const searchColumn = ref<string | null>(null)
const showModal = ref(false)
const modalMode = ref<'create' | 'edit'>('create')
const formData = ref<Record<string, any>>({})
const originalRow = ref<Record<string, any>>({})
const submitting = ref(false)
const primaryKeys = computed(() => tableMetadata.value.filter(c => c.is_pk).map(c => c.name as string))

// An async result must never populate a different connection/database/table.
const targetKey = computed(() => JSON.stringify([props.config, props.database, props.table]))
let dataRequest = 0
let schemaReady = false
let disposed = false
function captureTarget() {
    return {
        key: targetKey.value,
        table: props.table,
        config: { ...props.config, database: props.database ?? props.config.database },
        dialect: sqlDialect(props.config.db_type)
    }
}
type Target = ReturnType<typeof captureTarget>
let modalTarget: Target | null = null
function isCurrent(target: Target) { return !disposed && target.key === targetKey.value }
onBeforeUnmount(() => { disposed = true; dataRequest++ })

const searchColumnOptions = computed(() => [
    { label: t('manage.all_columns'), value: '__all__' },
    ...tableMetadata.value.map(col => ({ label: col.name, value: col.name }))
])
const renderColumnSelectLabel = (option: any) => h(NEllipsis, { tooltip: true }, { default: () => option.label })
const exportOptions = [
    { label: 'CSV', key: 'csv' },
    { label: 'JSON', key: 'json' },
    { label: 'SQL (INSERT)', key: 'sql' }
]
const tableColumns = ref<DataTableColumns>([])

watch(tableMetadata, (newMeta) => {
    tableColumns.value = [
        ...newMeta.map(col => ({
            title() {
                return h('div', { style: 'display: flex; flex-direction: column; align-items: start; width: 100%; overflow: hidden;' }, [
                    h(NEllipsis, { tooltip: true, style: 'font-weight: 500; max-width: 100%;' }, { default: () => col.name }),
                    col.comment ? h(NEllipsis, { tooltip: true, style: 'font-size: 12px; color: #999; margin-top: 2px; max-width: 100%;' }, { default: () => col.comment }) : null
                ])
            },
            key: col.name,
            resizable: true,
            minWidth: 50,
            maxWidth: 1000,
            width: Math.max(120, Math.min(300, col.name.length * 10 + 40)),
            ellipsis: { tooltip: true },
            sorter: true,
            sortOrder: sortColumn.value === col.name ? sortOrder.value : false,
            render(row: any) {
                let val = row[col.name]
                if (val === null) return h('span', { style: 'color: #ccc; font-style: italic;' }, '[NULL]')
                let isJson = typeof val === 'object' && val !== null
                if (typeof val === 'string' && val.trim()) {
                    const trimmed = val.trim()
                    if ((trimmed.startsWith('{') && trimmed.endsWith('}')) || (trimmed.startsWith('[') && trimmed.endsWith(']'))) {
                        try { val = JSON.parse(val); isJson = true } catch { /* not JSON */ }
                    }
                }
                if (isJson) {
                    const fullStr = JSON.stringify(val)
                    const preview = fullStr.length > 50 ? fullStr.slice(0, 50) + '...' : fullStr
                    return h('span', { style: 'color: #18a058; cursor: default;', title: JSON.stringify(val, null, 2) }, preview)
                }
                if (typeof val === 'string' && val.length > 100) {
                    return h('span', { style: 'cursor: default;', title: val }, val.slice(0, 80) + '...')
                }
                return String(val)
            }
        })),
        {
            title: t('common.edit'),
            key: 'actions',
            width: 120,
            render(row: any) {
                return h(NSpace, { size: 'small' }, {
                    default: () => [
                        h(NButton, { size: 'tiny', quaternary: true, onClick: () => openEdit(row) },
                            { icon: () => h(NIcon, null, { default: () => h(CreateOutline) }) }),
                        h(NButton, { size: 'tiny', quaternary: true, type: 'error', onClick: () => handleDelete(row) },
                            { icon: () => h(NIcon, null, { default: () => h(TrashOutline) }) })
                    ]
                })
            }
        }
    ]
}, { immediate: true })

watch([sortColumn, sortOrder], () => {
    for (const column of tableColumns.value as any[]) {
        if (column.sorter) column.sortOrder = column.key === sortColumn.value ? sortOrder.value : false
    }
})

function handleColumnResized(width: number, colKey: string) {
    const col = tableColumns.value.find((c: any) => c.key === colKey)
    if (col) col.width = width
}

function rowKey(row: Record<string, any>): string {
    return JSON.stringify(primaryKeys.value.length ? primaryKeys.value.map(key => row[key]) : row)
}

function buildWhereClause(target: Target): string {
    return searchWhere(tableMetadata.value, searchColumn.value, searchKeyword.value, target.dialect)
}

function buildOrderBy(target: Target): string {
    if (!sortColumn.value || !sortOrder.value) return ''
    if (!tableMetadata.value.some(column => column.name === sortColumn.value)) return ''
    const direction = sortOrder.value === 'ascend' ? 'ASC' : 'DESC'
    return ` ORDER BY ${quoteIdentifier(sortColumn.value, target.dialect)} ${direction}`
}

async function loadSchema(target: Target): Promise<boolean> {
    try {
        const cols = await invoke<any[]>('get_columns', {
            config: target.config, table: target.table, database: target.config.database || null
        })
        if (!isCurrent(target)) return false
        tableMetadata.value = cols
        schemaReady = true
        return true
    } catch (e: any) {
        if (isCurrent(target)) message.error('Failed to load columns: ' + e.toString())
        return false
    }
}

async function loadData(target = captureTarget()) {
    if (!schemaReady || !target.table || !isCurrent(target)) return
    const request = ++dataRequest
    loading.value = true
    try {
        const offset = (page.value - 1) * pageSize.value
        const where = buildWhereClause(target)
        const table = quoteIdentifier(target.table, target.dialect)
        const countQuery = `SELECT COUNT(*) as cx FROM ${table}${where}`
        const dataQuery = `SELECT * FROM ${table}${where}${buildOrderBy(target)} LIMIT ${pageSize.value} OFFSET ${offset}`
        const [countRes, rows] = await Promise.all([
            invoke<any[]>('execute_query', { config: target.config, query: countQuery }),
            invoke<any[]>('execute_query', { config: target.config, query: dataQuery })
        ])
        if (request !== dataRequest || !isCurrent(target)) return
        total.value = Number(countRes[0]?.cx ?? countRes[0]?.count ?? 0)
        data.value = rows
    } catch (e: any) {
        if (request === dataRequest && isCurrent(target)) message.error('Failed to load data: ' + e.toString())
    } finally {
        if (request === dataRequest && isCurrent(target)) loading.value = false
    }
}

async function refresh() {
    if (!props.table) return
    const target = captureTarget()
    if (await loadSchema(target)) await loadData(target)
}

function handleSearch() {
    if (page.value !== 1) page.value = 1
    else void loadData()
}

watch(targetKey, () => {
    dataRequest++
    schemaReady = false
    loading.value = false
    tableMetadata.value = []
    data.value = []
    total.value = 0
    page.value = 1
    searchKeyword.value = ''
    searchColumn.value = null
    sortColumn.value = null
    sortOrder.value = false
    showModal.value = false
    modalTarget = null
    void refresh()
}, { immediate: true })
watch(page, () => { void loadData() })

function handleSorterChange(sorter: { columnKey: string, order: 'ascend' | 'descend' | false } | null) {
    sortColumn.value = sorter?.order ? sorter.columnKey : null
    sortOrder.value = sorter?.order || false
    handleSearch()
}

function editValue(value: any): any {
    return value !== null && typeof value === 'object' ? JSON.stringify(value) : value
}

function openCreate() {
    if (!guardBinaryValues()) return
    if (!schemaReady) return
    modalMode.value = 'create'
    modalTarget = captureTarget()
    formData.value = Object.fromEntries(tableMetadata.value.map(col => [col.name, null]))
    originalRow.value = {}
    showModal.value = true
}

function openEdit(row: any) {
    if (!guardBinaryValues()) return
    try {
        const target = captureTarget()
        primaryKeyWhere(tableMetadata.value, row, target.dialect)
        modalMode.value = 'edit'
        modalTarget = target
        originalRow.value = { ...row }
        formData.value = Object.fromEntries(Object.entries(row).map(([key, value]) => [key, inputValue(key, value)]))
        showModal.value = true
    } catch (e: any) { message.warning(e.toString()) }
}

function handleDelete(row: any) {
    if (!guardBinaryValues()) return
    try {
        const target = captureTarget()
        // Build from the original row before the confirmation dialog can outlive it.
        const query = deleteQuery(target.table, tableMetadata.value, row, target.dialect)
        const label = primaryKeys.value.map(key => `${key} = ${row[key]}`).join(', ')
        dialog.warning({
            title: t('common.delete'),
            content: `确定要删除这条记录吗？(${label})`,
            positiveText: t('common.delete'),
            negativeText: t('common.cancel'),
            onPositiveClick: async () => {
                if (!isCurrent(target)) { message.warning('Selection changed; delete cancelled.'); return false }
                try {
                    await invoke('execute_query', { config: target.config, query })
                    message.success(t('common.success'))
                    await loadData(target)
                } catch (e: any) { message.error('Delete failed: ' + e.toString()); return false }
            }
        })
    } catch (e: any) { message.warning(e.toString()) }
}

async function handleSubmit() {
    const target = modalTarget
    if (!target || !isCurrent(target)) { message.warning('Selection changed; save cancelled.'); return }
    submitting.value = true
    try {
        let query: string
        if (modalMode.value === 'create') {
            // Untouched null fields use database defaults; an entered empty string is data.
            const values = Object.fromEntries(Object.entries(formData.value).filter(([, value]) => value !== null))
            query = insertQuery(target.table, values, target.dialect)
        } else {
            const values = Object.fromEntries(Object.entries(formData.value).filter(([key, value]) =>
                !primaryKeys.value.includes(key) && value !== inputValue(key, originalRow.value[key])
            ))
            if (!Object.keys(values).length) { showModal.value = false; return }
            query = updateQuery(target.table, tableMetadata.value, originalRow.value, values, target.dialect)
        }
        await invoke('execute_query', { config: target.config, query })
        message.success(t('common.success'))
        if (isCurrent(target)) showModal.value = false
        await loadData(target)
    } catch (e: any) {
        message.error(t('common.error') + ': ' + e.toString())
    } finally { submitting.value = false }
}

function inputValue(name: string, value: any) {
    const column = tableMetadata.value.find(c => c.name === name)
    return value !== null && value !== undefined && column && /BIGINT|INT8|DECIMAL|NUMERIC/i.test(column.type_name)
        ? String(value) : editValue(value)
}
function hasBinaryColumns() { return tableMetadata.value.some(c => /binary|blob|bytea|bit\b/i.test(c.type_name)) }
function guardBinaryValues() {
    if (!hasBinaryColumns()) return true
    message.warning('Binary columns require a typed binary workflow. Use the SQL console or database-level SQL export; text import/export/editing is disabled to prevent corruption.')
    return false
}
async function handleExport(key: string) {
    if (!guardBinaryValues()) return
    try {
        if (!schemaReady || !['csv', 'json', 'sql'].includes(key)) return
        const target = captureTarget()
        const columns = tableMetadata.value.map(c => c.name as string)
        const query = `SELECT * FROM ${quoteIdentifier(target.table, target.dialect)}${buildWhereClause(target)}${buildOrderBy(target)}`
        const allRows = await invoke<any[]>('execute_query', { config: target.config, query })
        if (!allRows?.length) { message.warning(t('manage.export_no_data')); return }
        let content = ''
        if (key === 'csv') {
            const csvField = (value: string) => `"${value.replace(/"/g, '""')}"`
            const header = columns.map(csvField).join(',')
            const rows = allRows.map(row => columns.map(col => {
                const value = row[col]
                if (value === null || value === undefined) return ''
                return csvField(typeof value === 'object' ? JSON.stringify(value) : String(value))
            }).join(','))
            content = [header, ...rows].join('\n')
        } else if (key === 'json') content = JSON.stringify(allRows, null, 2)
        else content = allRows.map(row => {
            const values = Object.fromEntries(columns.map(column => [column, row[column]]))
            return `${insertQuery(target.table, values, target.dialect)};`
        }).join('\n')
        const filePath = await save({
            defaultPath: `${target.table}.${key}`,
            filters: [{ name: key.toUpperCase(), extensions: [key] }]
        })
        if (!filePath) return
        await writeTextFile(filePath, content)
        message.success(t('manage.export_success', { count: allRows.length }))
    } catch (e: any) { message.error(t('common.error') + ': ' + e.toString()) }
}

async function triggerImport() {
    if (!guardBinaryValues()) return
    if (!schemaReady || loading.value) return
    const target = captureTarget()
    const columnNames = new Set(tableMetadata.value.map(column => column.name))
    let successCount = 0
    try {
        const filePath = await open({ filters: [{ name: 'Data', extensions: ['csv', 'json'] }], multiple: false })
        if (!filePath) return
        if (!isCurrent(target)) throw new Error('Selection changed; import cancelled')
        loading.value = true
        const text = await readTextFile(filePath as string)
        const ext = (filePath as string).split('.').pop()?.toLowerCase()
        let rows: Record<string, any>[]
        if (ext === 'json') {
            const parsed = JSON.parse(text)
            rows = Array.isArray(parsed) ? parsed : [parsed]
        } else if (ext === 'csv') rows = parseCSV(text)
        else throw new Error('支持 CSV / JSON 格式')
        if (!rows.length) { message.warning('文件中没有数据'); return }
        // Validate the entire input before the first write. Do not omit null/empty data.
        rows.forEach(row => {
            if (!row || typeof row !== 'object' || Array.isArray(row)) throw new Error('Each imported row must be an object')
            const unknown = Object.keys(row).filter(name => !columnNames.has(name))
            if (unknown.length) throw new Error(`Unknown columns: ${unknown.join(', ')}`)
            for (const column of tableMetadata.value) {
                const value = row[column.name]
                if (typeof value === 'number' && (/BIGINT|INT8|DECIMAL|NUMERIC/i.test(column.type_name) || (Number.isInteger(value) && !Number.isSafeInteger(value)))) {
                    throw new Error(`Import ${column.name} as a JSON string to preserve exact numeric digits`)
                }
            }
            return insertQuery(target.table, row, target.dialect)
        })
        if (!isCurrent(target)) throw new Error('Selection changed; import cancelled')
        successCount = await invoke<number>('import_table_rows', { config: target.config, table: target.table, rows })
        message.success(t('manage.import_success', { count: successCount }))
    } catch (e: any) {
        // The backend reports rollback or an uncertain commit; never imply that retry is safe.
        message.error(`${t('manage.import_failed')}: ${e.toString()}`)
    } finally {
        if (isCurrent(target)) { loading.value = false; await loadData(target) }
    }
}
</script>

<template>
  <div class="data-grid">
      <!-- Toolbar Row 1: Actions -->
      <NSpace justify="space-between" class="toolbar" style="flex-wrap: wrap; gap: 8px;">
          <NSpace>
              <NButton @click="refresh" size="small">
                  <template #icon><NIcon><RefreshOutline /></NIcon></template>
              </NButton>
              <NButton type="primary" size="small" @click="openCreate">
                  <template #icon><NIcon><AddOutline /></NIcon></template>
                  {{ t('manage.add_row') }}
              </NButton>
              <NDropdown :options="exportOptions" @select="handleExport" trigger="click">
                  <NButton size="small">
                      <template #icon><NIcon><DownloadOutline /></NIcon></template>
                      {{ t('manage.export_table') }}
                  </NButton>
              </NDropdown>
              <NButton size="small" @click="triggerImport">
                  <template #icon><NIcon><CloudUploadOutline /></NIcon></template>
                  {{ t('manage.import_table') }}
              </NButton>
          </NSpace>
          <NSpace align="center">
              <NInputGroup style="width: 400px;">
                  <NSelect
                    v-model:value="searchColumn"
                    :options="searchColumnOptions"
                    :render-label="renderColumnSelectLabel"
                    size="small"
                    style="width: 180px;"
                    :placeholder="t('manage.all_columns')"
                    clearable
                  />
                  <NInput
                    v-model:value="searchKeyword"
                    size="small"
                    :placeholder="t('manage.search_placeholder')"
                    clearable
                    @keyup.enter="handleSearch"
                  >
                      <template #suffix>
                          <NIcon :component="SearchOutline" style="cursor: pointer;" @click="handleSearch" />
                      </template>
                  </NInput>
              </NInputGroup>
              <NSelect
                v-model:value="pageSize"
                :options="pageSizeOptions"
                size="small"
                style="width: 100px;"
                @update:value="handleSearch"
              />
              <NPagination
                v-model:page="page"
                :item-count="total"
                :page-size="pageSize"
                simple
                size="small"
              />
          </NSpace>
      </NSpace>

      <div class="table-container">
           <NDataTable
            :columns="tableColumns"
            :data="data"
            :loading="loading"
            flex-height
            remote
            :row-key="rowKey"
            style="height: 100%"
            size="small"
            :bordered="false"
            :scroll-x="tableMetadata.length * 150 + 100"
            @update:sorter="handleSorterChange"
            @update:columns="(cols: DataTableColumns) => { tableColumns = cols }"
            @resizable-column-resize="handleColumnResized"
          />
      </div>

    <!-- Edit/Create Modal -->
    <NModal v-model:show="showModal" preset="dialog" :title="modalMode === 'create' ? t('manage.add_row') : t('common.edit')">
        <NForm label-placement="left" label-width="auto" style="max-height: 500px; overflow-y: auto;">
             <NFormItem v-for="col in tableMetadata" :key="col.name" :path="col.name">
                 <template #label>
                    <NSpace align="center" size="small">
                        <span>{{ col.name }}</span>
                        <span v-if="col.comment" style="color: #999; font-size: 12px;">({{ col.comment }})</span>
                    </NSpace>
                 </template>
                 <NCheckbox v-if="col.type_name.toUpperCase().includes('BOOL')" v-model:checked="formData[col.name]" :disabled="modalMode === 'edit' && col.is_pk" />
                 <NInput v-else-if="/BIGINT|INT8|DECIMAL|NUMERIC/i.test(col.type_name) || typeof formData[col.name] === 'string'" v-model:value="formData[col.name]" :disabled="modalMode === 'edit' && col.is_pk" />
                 <NInputNumber v-else-if="['INT', 'FLOAT', 'DOUBLE', 'REAL'].some(t => col.type_name.toUpperCase().includes(t))" v-model:value="formData[col.name]" :disabled="modalMode === 'edit' && col.is_pk" />
                 <NInput v-else v-model:value="formData[col.name]" :disabled="modalMode === 'edit' && col.is_pk" placeholder="Raw value" />
             </NFormItem>
        </NForm>
        <template #action>
            <NButton @click="showModal = false">{{ t('common.cancel') }}</NButton>
            <NButton type="primary" :loading="submitting" @click="handleSubmit">{{ t('common.save') }}</NButton>
        </template>
    </NModal>
  </div>
</template>

<style scoped>
.data-grid {
    display: flex;
    flex-direction: column;
    height: 100%;
}
.toolbar {
    margin-bottom: 8px;
    padding-right: 12px;
}
.table-container {
    flex: 1;
    min-height: 0;
    box-sizing: border-box;
}
:deep(.n-data-table .n-data-table-base-table-body) {
    will-change: transform;
}
:deep(.n-data-table .n-data-table-base-table-header) {
    will-change: transform;
}
</style>
