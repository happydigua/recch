<script setup lang="ts">
import { ref, watch, computed, h } from 'vue'
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

// Search state
const searchKeyword = ref('')
const searchColumn = ref<string | null>(null) // null = all columns

// CRUD Modal
const showModal = ref(false)
const modalMode = ref<'create' | 'edit'>('create')
const formData = ref<Record<string, any>>({})
const submitting = ref(false)

// Primary Key for edits
const primaryKey = computed(() => {
    const pkCol = tableMetadata.value.find(c => c.is_pk)
    return pkCol ? pkCol.name : null
})

// Search column options
const searchColumnOptions = computed(() => {
    return [
        { label: t('manage.all_columns'), value: '__all__' },
        ...tableMetadata.value.map(col => ({ label: col.name, value: col.name }))
    ]
})

const renderColumnSelectLabel = (option: any) => {
    return h(NEllipsis, { tooltip: true }, { default: () => option.label })
}

// Export dropdown options
const exportOptions = [
    { label: 'CSV', key: 'csv' },
    { label: 'JSON', key: 'json' },
    { label: 'SQL (INSERT)', key: 'sql' }
]

const tableColumns = ref<DataTableColumns>([])

// Update columns definition whenever table metadata changes
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
                let val = row[col.name];
                
                if (val === null) {
                    return h('span', { style: 'color: #ccc; font-style: italic;' }, '[NULL]')
                }

                let isJson = false;
                if (typeof val === 'object' && val !== null) {
                    isJson = true;
                } else if (typeof val === 'string' && val.trim()) {
                    const trimmed = val.trim();
                    if ((trimmed.startsWith('{') && trimmed.endsWith('}')) || 
                        (trimmed.startsWith('[') && trimmed.endsWith(']'))) {
                        try {
                            val = JSON.parse(val);
                            isJson = true;
                        } catch (e) { /* not JSON */ }
                    }
                }
                
                if (isJson) {
                   const fullStr = JSON.stringify(val);
                   const preview = fullStr.length > 50 ? fullStr.slice(0, 50) + '...' : fullStr;
                   return h('span', { 
                       style: 'color: #18a058; cursor: default;',
                       title: JSON.stringify(val, null, 2)
                   }, preview);
                }
                
                if (typeof val === 'string' && val.length > 100) {
                   return h('span', { 
                       style: 'cursor: default;',
                       title: val 
                   }, val.slice(0, 80) + '...');
                }
                
                return String(val);
            }
        })),
        {
            title: t('common.edit'), 
            key: 'actions',
            width: 120,
            render(row: any) {
                return h(NSpace, { size: 'small' }, {
                    default: () => [
                        h(NButton, {
                            size: 'tiny',
                            quaternary: true,
                            onClick: () => openEdit(row)
                        }, { icon: () => h(NIcon, null, { default: () => h(CreateOutline) }) }),
                        h(NButton, {
                            size: 'tiny',
                            quaternary: true,
                            type: 'error',
                            onClick: () => handleDelete(row)
                        }, { icon: () => h(NIcon, null, { default: () => h(TrashOutline) }) })
                    ]
                })
            }
        }
    ]
}, { immediate: true })

// Handle column width dragging manually if naive-ui needs it
function handleColumnResized(width: number, colKey: string) {
    const col = tableColumns.value.find((c: any) => c.key === colKey)
    if (col) {
        col.width = width
    }
}

// Build WHERE clause from search
function buildWhereClause(): string {
    if (!searchKeyword.value.trim()) return ''
    const keyword = searchKeyword.value.trim().replace(/'/g, "''")
    
    if (searchColumn.value && searchColumn.value !== '__all__') {
        return ` WHERE \`${searchColumn.value}\` LIKE '%${keyword}%'`
    }
    
    // Search all columns
    const conditions = tableMetadata.value
        .map(col => `\`${col.name}\` LIKE '%${keyword}%'`)
        .join(' OR ')
    return conditions ? ` WHERE (${conditions})` : ''
}

async function loadSchema() {
    try {
        const cols = await invoke<any[]>('get_columns', {
            config: props.config,
            table: props.table,
            database: props.database || null
        })
        tableMetadata.value = cols
    } catch (e: any) {
        message.error('Failed to load columns: ' + e.toString())
    }
}

async function loadData() {
    loading.value = true
    try {
        const offset = (page.value - 1) * pageSize.value
        const limit = pageSize.value
        
        const where = buildWhereClause()
        const countQuery = `SELECT COUNT(*) as cx FROM ${props.table}${where}`

        let orderBy = ''
        if (sortColumn.value && sortOrder.value) {
            const direction = sortOrder.value === 'ascend' ? 'ASC' : 'DESC'
            orderBy = ` ORDER BY \`${sortColumn.value}\` ${direction}`
        }

        const dataQuery = `SELECT * FROM ${props.table}${where}${orderBy} LIMIT ${limit} OFFSET ${offset}`

        const [countRes, rows] = await Promise.all([
            invoke<any[]>('execute_query', { config: props.config, query: countQuery }),
            invoke<any[]>('execute_query', { config: props.config, query: dataQuery })
        ])

        if (countRes.length > 0) {
            total.value = Number(countRes[0].cx || countRes[0].count || 0)
        }
        data.value = rows
    } catch (e: any) {
        message.error('Failed to load data: ' + e.toString())
    } finally {
        loading.value = false
    }
}

async function refresh() {
    if (!props.table) return
    await loadSchema()
    await loadData()
}

function handleSearch() {
    page.value = 1
    loadData()
}

watch(() => props.table, () => {
    page.value = 1
    searchKeyword.value = ''
    searchColumn.value = null
    refresh()
}, { immediate: true })

watch(page, loadData)

function handleSorterChange(sorter: { columnKey: string, order: 'ascend' | 'descend' | false } | null) {
    if (sorter && sorter.order) {
        sortColumn.value = sorter.columnKey
        sortOrder.value = sorter.order
    } else {
        sortColumn.value = null
        sortOrder.value = false
    }
    page.value = 1
    loadData()
}

function openCreate() {
    modalMode.value = 'create'
    formData.value = {}
    tableMetadata.value.forEach(col => {
        formData.value[col.name] = null
    })
    showModal.value = true
}

function openEdit(row: any) {
    if (!primaryKey.value) {
        message.warning('Cannot edit: No Primary Key detected.')
        return
    }
    modalMode.value = 'edit'
    formData.value = { ...row } 
    showModal.value = true
}

async function handleDelete(row: any) {
     if (!primaryKey.value) {
        message.warning('Cannot delete: No Primary Key detected.')
        return
    }
    const pk = primaryKey.value
    const val = row[pk]
    
    dialog.warning({
        title: t('common.delete'),
        content: `确定要删除这条记录吗？(${pk} = ${val})`,
        positiveText: t('common.delete'),
        negativeText: t('common.cancel'),
        onPositiveClick: async () => {
            const valSql = typeof val === 'string' ? `'${val}'` : val
            const query = `DELETE FROM ${props.table} WHERE ${pk} = ${valSql}`
            
            try {
                loading.value = true
                await invoke('execute_query', { config: props.config, query })
                message.success(t('common.success'))
                loadData()
            } catch(e: any) {
                 message.error('Delete failed: ' + e.toString())
            } finally {
                loading.value = false
            }
        }
    })
}

async function handleSubmit() {
    submitting.value = true
    try {
        if (modalMode.value === 'create') {
            const cols = Object.keys(formData.value).filter(k => formData.value[k] !== null && formData.value[k] !== '')
            const vals = cols.map(k => {
                const v = formData.value[k]
                return typeof v === 'string' ? `'${v.replace(/'/g, "''")}'` : v
            })
            
            const query = `INSERT INTO ${props.table} (${cols.join(', ')}) VALUES (${vals.join(', ')})`
            await invoke('execute_query', { config: props.config, query })
            message.success(t('common.success'))
        } else {
            const pk = primaryKey.value!
            const pkVal = formData.value[pk]
            const pkValSql = typeof pkVal === 'string' ? `'${pkVal}'` : pkVal
            
            const updates = Object.keys(formData.value)
                .filter(k => k !== pk)
                .map(k => {
                    const v = formData.value[k]
                    const vSql = v === null ? 'NULL' : (typeof v === 'string' ? `'${v.replace(/'/g, "''")}'` : v)
                    return `${k} = ${vSql}`
                })
            
            const query = `UPDATE ${props.table} SET ${updates.join(', ')} WHERE ${pk} = ${pkValSql}`
             await invoke('execute_query', { config: props.config, query })
             message.success(t('common.success'))
        }
        showModal.value = false
        loadData()
    } catch (e: any) {
        message.error(t('common.error') + ': ' + e.toString())
    } finally {
        submitting.value = false
    }
}

// ============ Export ============

async function handleExport(key: string) {
    try {
        // Fetch all data (no pagination limit) with current search
        const where = buildWhereClause()
        let orderBy = ''
        if (sortColumn.value && sortOrder.value) {
            const direction = sortOrder.value === 'ascend' ? 'ASC' : 'DESC'
            orderBy = ` ORDER BY \`${sortColumn.value}\` ${direction}`
        }
        const query = `SELECT * FROM ${props.table}${where}${orderBy}`
        const allRows = await invoke<any[]>('execute_query', { config: props.config, query })
        
        if (!allRows || allRows.length === 0) {
            message.warning(t('manage.export_no_data'))
            return
        }

        let content = ''
        const columns = tableMetadata.value.map(c => c.name)
        let defaultName = ''
        let filterName = ''
        let filterExt: string[] = []

        if (key === 'csv') {
            const header = columns.map(c => `"${c}"`).join(',')
            const rows = allRows.map(row => 
                columns.map(col => {
                    const val = row[col]
                    if (val === null || val === undefined) return ''
                    const str = typeof val === 'object' ? JSON.stringify(val) : String(val)
                    return `"${str.replace(/"/g, '""')}"`
                }).join(',')
            )
            content = [header, ...rows].join('\n')
            defaultName = `${props.table}.csv`
            filterName = 'CSV'
            filterExt = ['csv']
        } else if (key === 'json') {
            content = JSON.stringify(allRows, null, 2)
            defaultName = `${props.table}.json`
            filterName = 'JSON'
            filterExt = ['json']
        } else if (key === 'sql') {
            const statements = allRows.map(row => {
                const cols = columns.filter(c => row[c] !== null && row[c] !== undefined)
                const vals = cols.map(c => {
                    const v = row[c]
                    if (typeof v === 'object') return `'${JSON.stringify(v).replace(/'/g, "''")}'`
                    if (typeof v === 'string') return `'${v.replace(/'/g, "''")}'`
                    return String(v)
                })
                return `INSERT INTO ${props.table} (${cols.map(c => `\`${c}\``).join(', ')}) VALUES (${vals.join(', ')});`
            })
            content = statements.join('\n')
            defaultName = `${props.table}.sql`
            filterName = 'SQL'
            filterExt = ['sql']
        }

        // Use Tauri save dialog
        const filePath = await save({
            defaultPath: defaultName,
            filters: [{ name: filterName, extensions: filterExt }]
        })

        if (!filePath) return // User cancelled

        await writeTextFile(filePath, content)
        message.success(t('manage.export_success', { count: allRows.length }))
    } catch (e: any) {
        message.error(t('common.error') + ': ' + e.toString())
    }
}

// ============ Import ============

async function triggerImport() {
    try {
        const filePath = await open({
            filters: [{ name: 'Data', extensions: ['csv', 'json'] }],
            multiple: false
        })
        if (!filePath) return

        loading.value = true
        const text = await readTextFile(filePath as string)
        let rows: Record<string, any>[] = []
        const path = filePath as string
        const ext = path.split('.').pop()?.toLowerCase()

        if (ext === 'json') {
            const parsed = JSON.parse(text)
            rows = Array.isArray(parsed) ? parsed : [parsed]
        } else if (ext === 'csv') {
            rows = parseCSV(text)
        } else {
            message.error('支持 CSV / JSON 格式')
            return
        }

        if (rows.length === 0) {
            message.warning('文件中没有数据')
            return
        }

        let successCount = 0
        for (const row of rows) {
            const cols = Object.keys(row).filter(k => row[k] !== null && row[k] !== undefined && row[k] !== '')
            if (cols.length === 0) continue

            const vals = cols.map(c => {
                const v = row[c]
                if (typeof v === 'object') return `'${JSON.stringify(v).replace(/'/g, "''")}'`
                if (typeof v === 'string') return `'${v.replace(/'/g, "''")}'`
                return String(v)
            })
            
            const query = `INSERT INTO ${props.table} (${cols.map(c => `\`${c}\``).join(', ')}) VALUES (${vals.join(', ')})`
            try {
                await invoke('execute_query', { config: props.config, query })
                successCount++
            } catch (e: any) {
                console.error(`Import row failed:`, e)
            }
        }

        message.success(t('manage.import_success', { count: successCount }))
        loadData()
    } catch (e: any) {
        message.error(t('manage.import_failed') + ': ' + e.toString())
    } finally {
        loading.value = false
    }
}

function parseCSV(text: string): Record<string, any>[] {
    const lines = text.split('\n').filter(line => line.trim())
    if (lines.length < 2) return []

    // Parse header
    const headers = parseCSVLine(lines[0]!)
    
    const result: Record<string, any>[] = []
    for (let i = 1; i < lines.length; i++) {
        const values = parseCSVLine(lines[i]!)
        const row: Record<string, any> = {}
        headers.forEach((h, idx) => {
            row[h] = values[idx] !== undefined ? values[idx] : null
        })
        result.push(row)
    }
    return result
}

function parseCSVLine(line: string): string[] {
    const result: string[] = []
    let current = ''
    let inQuotes = false
    
    for (let i = 0; i < line.length; i++) {
        const char = line[i]
        if (inQuotes) {
            if (char === '"') {
                if (i + 1 < line.length && line[i + 1] === '"') {
                    current += '"'
                    i++
                } else {
                    inQuotes = false
                }
            } else {
                current += char
            }
        } else {
            if (char === '"') {
                inQuotes = true
            } else if (char === ',') {
                result.push(current.trim())
                current = ''
            } else {
                current += char
            }
        }
    }
    result.push(current.trim())
    return result
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
                @update:value="() => { page = 1; loadData() }"
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
            :row-key="(row) => primaryKey ? row[primaryKey] : (row.id || Object.values(row).join('-'))"
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
                 <NInput v-if="['VARCHAR', 'TEXT', 'CHAR'].some(t => col.type_name.includes(t))" v-model:value="formData[col.name]"  />
                 <NInputNumber v-else-if="['INT', 'FLOAT', 'DOUBLE', 'DECIMAL'].some(t => col.type_name.includes(t))" v-model:value="formData[col.name]" />
                 <NCheckbox v-else-if="['BOOL', 'TINYINT'].some(t => col.type_name.includes(t))" v-model:checked="formData[col.name]" />
                 <NInput v-else v-model:value="formData[col.name]" placeholder="Raw value" />
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
