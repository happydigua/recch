<script setup lang="ts">
import { ref, watch, computed, h } from 'vue'
import { 
  NDataTable, NButton, NSpace, NIcon, NPagination, useMessage, useDialog,
  NModal, NForm, NFormItem, NInput, NInputNumber, NCheckbox, NSelect, NPopover
} from 'naive-ui'
import { 
  AddOutline, RefreshOutline, TrashOutline, CreateOutline 
} from '@vicons/ionicons5'
import { invoke } from '../utils/tauri'
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

// Columns for the data table
const tableColumns = computed<DataTableColumns>(() => {
    return [
        ...tableMetadata.value.map(col => ({
            title() {
                // Custom header render to show comment
                return h('div', { style: 'display: flex; flex-direction: column; align-items: start;' }, [
                    h('span', { style: 'font-weight: 500;' }, col.name),
                    col.comment ? h('span', { style: 'font-size: 12px; color: #999; margin-top: 2px;' }, col.comment) : null
                ])
            },
            key: col.name,
            width: 150,
            // Disable default ellipsis tooltip for JSON/Text types, let our custom renderer handle it
            ellipsis: !['json', 'text'].some(t => col.type_name?.toLowerCase().includes(t)) ? { tooltip: true } : undefined,
            sorter: true,
            sortOrder: sortColumn.value === col.name ? sortOrder.value : false,
            render(row: any) {
                let val = row[col.name];
                
                // Try to detect and parse JSON strings (in TEXT/VARCHAR fields too)
                let parsedJson = null;
                if (typeof val === 'string' && val.trim()) {
                    const trimmed = val.trim();
                    // Check if it looks like JSON (starts with { or [)
                    if ((trimmed.startsWith('{') && trimmed.endsWith('}')) || 
                        (trimmed.startsWith('[') && trimmed.endsWith(']'))) {
                        try {
                            parsedJson = JSON.parse(val);
                        } catch (e) {
                            // Not valid JSON, treat as regular string
                        }
                    }
                }
                
                // Use parsed JSON if available
                if (parsedJson !== null) {
                    val = parsedJson;
                }

                if (val === null) {
                    return h('span', { style: 'color: #ccc; font-style: italic;' }, '[NULL]')
                }

                // Render as JSON popover if it's an object or was detected as JSON
                if (typeof val === 'object' && val !== null) {
                   const fullStr = JSON.stringify(val);
                   const preview = fullStr.length > 50 ? fullStr.slice(0, 50) + '...' : fullStr;
                   const formatted = JSON.stringify(val, null, 2);
                   
                   return h(NPopover, { 
                       trigger: 'hover', 
                       placement: 'right-start', 
                       style: { padding: 0, backgroundColor: '#e7f5ee', border: '1px solid #18a058' },
                       arrowStyle: { backgroundColor: '#e7f5ee' }
                   }, {
                       trigger: () => h('div', { 
                           style: 'max-width: 150px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; color: #18a058;' 
                       }, preview),
                       default: () => h('div', {
                           style: 'background-color: #e7f5ee; color: #18a058; padding: 12px; border-radius: 4px; max-height: 60vh; max-width: 400px; overflow-y: auto;' 
                       }, [
                           h('pre', { 
                               style: 'margin: 0; font-family: monospace; white-space: pre-wrap; font-size: 12px;' 
                           }, formatted)
                       ])
                   });
                }
                
                // Truncate long text strings (> 100 chars) with hover to see full content
                if (typeof val === 'string' && val.length > 100) {
                   const preview = val.slice(0, 80) + '...';
                   
                   return h(NPopover, { 
                       trigger: 'hover', 
                       placement: 'right-start', 
                       style: { padding: 0, maxWidth: '500px' }
                   }, {
                       trigger: () => h('div', { 
                           style: 'max-width: 150px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; color: #666;' 
                       }, preview),
                       default: () => h('div', {
                           style: 'padding: 12px; max-height: 60vh; max-width: 480px; overflow-y: auto;' 
                       }, [
                           h('pre', { 
                               style: 'margin: 0; white-space: pre-wrap; font-size: 12px; word-break: break-all;' 
                           }, val)
                       ])
                   });
                }
                
                return val;
            }
        })),
        {
            title: t('common.edit'), 
            key: 'actions',
            width: 150,
            fixed: 'right' as const,
            render(row: any) {
                return h(NSpace, null, {
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
})

async function loadSchema() {
    try {
        const cols = await invoke<any[]>('get_columns', { 
            config: props.config, 
            table: props.table,
            database: props.database
        })
        tableMetadata.value = cols
    } catch (e) {
        console.error('get_columns error:', e)
        message.error(t('common.error') + ': ' + String(e))
    }
}

async function loadData() {
    loading.value = true
    try {
        const offset = (page.value - 1) * pageSize.value
        const limit = pageSize.value
        
        const countQuery = `SELECT COUNT(*) as cx FROM ${props.table}`
        const countRes = await invoke<any[]>('execute_query', {
             config: props.config, 
             query: countQuery 
        })
        if (countRes.length > 0) {
            total.value = Number(countRes[0].cx || countRes[0].count || 0)
        }

        // Build ORDER BY clause if sorting is active
        let orderBy = ''
        if (sortColumn.value && sortOrder.value) {
            const direction = sortOrder.value === 'ascend' ? 'ASC' : 'DESC'
            orderBy = ` ORDER BY \`${sortColumn.value}\` ${direction}`
        }

        const dataQuery = `SELECT * FROM ${props.table}${orderBy} LIMIT ${limit} OFFSET ${offset}`
        const rows = await invoke<any[]>('execute_query', {
             config: props.config,
             query: dataQuery
        })
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

watch(() => props.table, () => {
    page.value = 1
    refresh()
}, { immediate: true })

watch(page, loadData)

// Handle server-side sorting
function handleSorterChange(sorter: { columnKey: string, order: 'ascend' | 'descend' | false } | null) {
    if (sorter && sorter.order) {
        sortColumn.value = sorter.columnKey
        sortOrder.value = sorter.order
    } else {
        sortColumn.value = null
        sortOrder.value = false
    }
    page.value = 1 // Reset to first page when sorting
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
    
    // Show confirmation dialog
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
</script>

<template>
  <div class="data-grid">
      <NSpace justify="space-between" class="toolbar" style="flex-wrap: wrap; gap: 8px;">
          <NSpace>
              <NButton @click="refresh" size="small">
                  <template #icon><NIcon><RefreshOutline /></NIcon></template>
              </NButton>
              <NButton type="primary" size="small" @click="openCreate">
                  <template #icon><NIcon><AddOutline /></NIcon></template>
                  {{ t('manage.add_row') }}
              </NButton>
          </NSpace>
          <NSpace align="center">
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
    /* padding-right removed to let table fill space organically */
    box-sizing: border-box;
}
</style>
