<script setup lang="ts">
import { ref, computed, watch, h } from 'vue'
import { 
  NButton, NDataTable, NSpace, NIcon, useMessage, useDialog, 
  NModal, NForm, NFormItem, NInput, NSelect, NTabs, NTabPane, NCheckbox 
} from 'naive-ui'
import { AddOutline, RefreshOutline, TrashOutline, CreateOutline } from '@vicons/ionicons5'
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

interface ColumnDef {
    name: string
    type_name: string
    is_pk: boolean
    is_nullable?: boolean
    default_value?: string
    comment?: string
}

interface IndexDef {
    name: string
    columns: string[]
    is_unique: boolean
    is_pk: boolean
    comment?: string
}

const loading = ref(false)
const columns = ref<ColumnDef[]>([])

const loadingIndexes = ref(false)
const indexes = ref<IndexDef[]>([])

const showModal = ref(false)
const modalMode = ref<'add' | 'edit'>('add')
const formModel = ref({
    name: '',
    type_name: 'VARCHAR(255)',
    is_pk: false,
    is_nullable: true,
    default_value: '',
    comment: ''
})
const originalName = ref('')

const showIndexModal = ref(false)
const indexForm = ref({
    name: '',
    columns: [] as string[],
    is_unique: false
})

const typeOptions = [
    { label: 'INT', value: 'INT' },
    { label: 'BIGINT', value: 'BIGINT' },
    { label: 'VARCHAR(100)', value: 'VARCHAR(100)' },
    { label: 'VARCHAR(255)', value: 'VARCHAR(255)' },
    { label: 'TEXT', value: 'TEXT' },
    { label: 'DATE', value: 'DATE' },
    { label: 'DATETIME', value: 'DATETIME' },
    { label: 'TIMESTAMP', value: 'TIMESTAMP' },
    { label: 'BOOLEAN', value: 'BOOLEAN' },
    { label: 'FLOAT', value: 'FLOAT' },
    { label: 'DOUBLE', value: 'DOUBLE' }
]

const columnOptions = computed(() => {
    return columns.value.map(c => ({
        label: c.name,
        value: c.name
    }))
})

const gridColumns = computed<DataTableColumns<ColumnDef>>(() => [
    { title: t('structure.col_name'), key: 'name' },
    { title: t('structure.col_type'), key: 'type_name' },
    { 
        title: 'PK', 
        key: 'is_pk',
        render: (row) => row.is_pk ? '🔑' : ''
    },
    { 
        title: t('structure.nullable'), 
        key: 'is_nullable',
        render: (row) => row.is_nullable ? '✅' : '❌'
    },
    { title: t('structure.default_value'), key: 'default_value' },
    { title: t('structure.comment'), key: 'comment' },
    {
        title: t('common.edit'),
        key: 'actions',
        width: 150,
        fixed: 'right',
        render(row) {
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
                        disabled: row.is_pk, 
                        onClick: () => handleDrop(row)
                    }, { icon: () => h(NIcon, null, { default: () => h(TrashOutline) }) })
                ]
            })
        }
    }
])

const indexGridColumns = computed<DataTableColumns<IndexDef>>(() => [
    { title: t('structure.index_name'), key: 'name' },
    { 
        title: t('structure.columns'), 
        key: 'columns',
        render: (row) => row.columns.join(', ')
    },
    { 
        title: t('structure.unique'), 
        key: 'is_unique',
        render: (row) => row.is_unique ? '✅' : ''
    },
    {
        title: t('common.edit'),
        key: 'actions',
        width: 100,
        fixed: 'right',
        render(row) {
            return h(NSpace, null, {
                default: () => [
                     h(NButton, {
                        size: 'tiny',
                        quaternary: true,
                        type: 'error',
                        disabled: row.is_pk, 
                        onClick: () => handleDropIndex(row)
                    }, { icon: () => h(NIcon, null, { default: () => h(TrashOutline) }) })
                ]
            })
        }
    }
])

async function loadColumns() {
    loading.value = true
    try {
        columns.value = await invoke('get_columns', {
            config: props.config,
            table: props.table,
            database: props.database
        })
    } catch (e) {
        message.error(String(e))
    } finally {
        loading.value = false
    }
}

async function loadIndexes() {
    loadingIndexes.value = true
    try {
        indexes.value = await invoke('get_indexes', {
            config: props.config,
            table: props.table
        })
    } catch (e) {
        message.error(String(e))
    } finally {
        loadingIndexes.value = false
    }
}

watch(() => props.table, () => {
    loadColumns()
    loadIndexes()
}, { immediate: true })

function openAdd() {
    modalMode.value = 'add'
    formModel.value = { name: '', type_name: 'VARCHAR(255)', is_pk: false, is_nullable: true, default_value: '', comment: '' }
    showModal.value = true
}

function openEdit(row: ColumnDef) {
    modalMode.value = 'edit'
    originalName.value = row.name as string
    formModel.value = {
        name: row.name as string,
        type_name: row.type_name as string,
        is_pk: row.is_pk,
        is_nullable: row.is_nullable !== false, 
        default_value: (row.default_value || '') as string,
        comment: (row.comment || '') as string
    }
    showModal.value = true
}

function openAddIndex() {
    indexForm.value = { name: '', columns: [], is_unique: false }
    showIndexModal.value = true
}

async function handleIndexSubmit() {
    try {
        await invoke('alter_table', {
            config: props.config,
            table: props.table,
            operation: {
                op_type: 'add_index',
                column_name: '', // ignored
                index_def: {
                    name: indexForm.value.name,
                    columns: indexForm.value.columns,
                    is_unique: indexForm.value.is_unique,
                    is_pk: false,
                    comment: null
                }
            }
        })
        message.success(t('common.success'))
        showIndexModal.value = false
        loadIndexes()
    } catch (e: any) {
         message.error(t('common.error') + ': ' + e.toString())
    }
}

function handleDropIndex(row: IndexDef) {
     dialog.warning({
        title: t('common.delete'),
        content: `Drop index ${row.name}?`,
        positiveText: t('common.delete'),
        negativeText: t('common.cancel'),
        onPositiveClick: async () => {
             try {
                await invoke('alter_table', {
                    config: props.config,
                    table: props.table,
                    operation: {
                        op_type: 'drop_index',
                        index_name: row.name,
                        column_name: '' // ignored
                    }
                })
                message.success(t('common.success'))
                loadIndexes()
            } catch (e) {
                message.error(String(e))
            }
        }
    })
}

function handleDrop(row: ColumnDef) {
    dialog.warning({
        title: t('common.delete'),
        content: t('structure.drop_confirm', { name: row.name }),
        positiveText: t('common.delete'),
        negativeText: t('common.cancel'),
        onPositiveClick: async () => {
             try {
                await invoke('alter_table', {
                    config: props.config,
                    table: props.table,
                    operation: {
                        op_type: 'drop',
                        column_name: row.name,
                    }
                })
                message.success(t('common.success'))
                loadColumns()
            } catch (e) {
                message.error(String(e))
            }
        }
    })
}

async function handleSubmit() {
    try {
        if (modalMode.value === 'add') {
             await invoke('alter_table', {
                config: props.config,
                table: props.table,
                operation: {
                    op_type: 'add',
                    column_name: formModel.value.name,
                    column_def: {
                        name: formModel.value.name,
                        type_name: formModel.value.type_name,
                        is_pk: formModel.value.is_pk,
                        is_nullable: formModel.value.is_nullable,
                        default_value: formModel.value.default_value || null,
                        comment: formModel.value.comment || null
                    }
                }
            })
            message.success(t('common.success'))
        } else {
             // If name changed -> Rename
             if (formModel.value.name !== originalName.value) {
                  await invoke('alter_table', {
                    config: props.config,
                    table: props.table,
                    operation: {
                        op_type: 'rename',
                        column_name: originalName.value,
                        new_name: formModel.value.name
                    }
                })
             }
             
             // Modify
             await invoke('alter_table', {
                config: props.config,
                table: props.table,
                operation: {
                    op_type: 'modify',
                    column_name: formModel.value.name,
                    column_def: {
                        name: formModel.value.name,
                        type_name: formModel.value.type_name,
                        is_pk: false, // preserving non-pk assumption for now
                        is_nullable: formModel.value.is_nullable,
                        default_value: formModel.value.default_value || null,
                        comment: formModel.value.comment || null
                    }
                }
            })
            message.success(t('common.success'))
        }
        showModal.value = false
        loadColumns()
    } catch (e: any) {
        message.error(t('common.error') + ': ' + e.toString())
    }
}
</script>

<template>
    <div class="table-structure">
        <NTabs type="line" animated style="height: 100%; display: flex; flex-direction: column;">
            <NTabPane name="columns" :tab="t('structure.columnsMap') || 'Columns'">
                <div class="pane-content">
                    <NSpace justify="space-between" class="toolbar">
                         <NButton @click="loadColumns" size="small">
                              <template #icon><NIcon><RefreshOutline /></NIcon></template>
                          </NButton>
                          <NButton type="primary" size="small" @click="openAdd">
                              <template #icon><NIcon><AddOutline /></NIcon></template>
                              {{ t('structure.add_column') }}
                          </NButton>
                    </NSpace>
                    
                    <div class="table-container">
                        <NDataTable 
                            :columns="gridColumns" 
                            :data="columns" 
                            :loading="loading" 
                            flex-height
                            style="height: 100%"
                            size="small" 
                            :scroll-x="1000"
                        />
                    </div>
                </div>
            </NTabPane>
            
            <NTabPane name="indexes" :tab="t('structure.indexesMap') || 'Indexes'">
                 <div class="pane-content">
                    <NSpace justify="space-between" class="toolbar">
                         <NButton @click="loadIndexes" size="small">
                              <template #icon><NIcon><RefreshOutline /></NIcon></template>
                          </NButton>
                          <NButton type="primary" size="small" @click="openAddIndex">
                              <template #icon><NIcon><AddOutline /></NIcon></template>
                              {{ t('structure.add_index') }}
                          </NButton>
                    </NSpace>
                    
                    <div class="table-container">
                        <NDataTable 
                            :columns="indexGridColumns" 
                            :data="indexes" 
                            :loading="loadingIndexes" 
                            flex-height
                            style="height: 100%"
                            size="small" 
                        />
                    </div>
                </div>
            </NTabPane>
        </NTabs>

        <!-- Column Modal -->
        <NModal v-model:show="showModal" preset="dialog" :title="modalMode === 'add' ? t('structure.add_column') : t('structure.edit_column')">
             <NForm label-placement="left" label-width="auto">
                 <NFormItem :label="t('structure.col_name')" path="name">
                     <NInput v-model:value="formModel.name" />
                 </NFormItem>
                 <div style="display: flex; gap: 12px;">
                     <NFormItem :label="t('structure.col_type')" path="type_name" style="flex: 2;">
                          <NSelect 
                            v-model:value="formModel.type_name" 
                            filterable 
                            tag 
                            :options="typeOptions" 
                            placeholder="Select or type..." 
                         />
                     </NFormItem>
                     <NFormItem :label="t('structure.pk')" path="is_pk" style="flex: 1;">
                         <NCheckbox v-model:checked="formModel.is_pk" :disabled="modalMode === 'edit'" />
                     </NFormItem>
                 </div>

                 <!-- Default Value & Nullable -->
                 <div style="display: flex; gap: 12px;">
                     <NFormItem :label="t('structure.nullable')" path="is_nullable" style="flex: 1;">
                         <NCheckbox v-model:checked="formModel.is_nullable" />
                     </NFormItem>
                     <NFormItem :label="t('structure.default_value')" path="default_value" style="flex: 2;">
                         <NInput v-model:value="formModel.default_value" placeholder="NULL" />
                     </NFormItem>
                 </div>
                 
                 <NFormItem :label="t('structure.comment')" path="comment">
                     <NInput v-model:value="formModel.comment" type="textarea" :rows="2" />
                 </NFormItem>
             </NForm>
              <template #action>
                <NButton @click="showModal = false">{{ t('common.cancel') }}</NButton>
                <NButton type="primary" @click="handleSubmit">{{ t('common.save') }}</NButton>
            </template>
        </NModal>
        
        <!-- Index Modal -->
        <NModal v-model:show="showIndexModal" preset="dialog" :title="t('structure.add_index')">
             <NForm label-placement="left" label-width="auto">
                 <NFormItem :label="t('structure.index_name')" path="name">
                     <NInput v-model:value="indexForm.name" />
                 </NFormItem>
                 <NFormItem :label="t('structure.columns')" path="columns">
                      <NSelect 
                        v-model:value="indexForm.columns" 
                        multiple
                        :options="columnOptions" 
                        placeholder="Select columns..." 
                     />
                 </NFormItem>
                 <NFormItem :label="t('structure.unique')" path="is_unique">
                     <NCheckbox v-model:checked="indexForm.is_unique" />
                 </NFormItem>
             </NForm>
              <template #action>
                <NButton @click="showIndexModal = false">{{ t('common.cancel') }}</NButton>
                <NButton type="primary" @click="handleIndexSubmit">{{ t('common.save') }}</NButton>
            </template>
        </NModal>
    </div>
</template>

<style scoped>
.table-structure {
    height: 100%;
}
.pane-content {
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
:deep(.n-tabs-pane-wrapper) {
    height: 100%;
}
</style>
