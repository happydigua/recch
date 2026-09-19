<script setup lang="ts">
import { ref, computed, watch, h, onBeforeUnmount } from 'vue'
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

let generation = 0
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
                <NButton type="primary" :loading="submitting" @click="handleSubmit">{{ t('common.save') }}</NButton>
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
                <NButton type="primary" :loading="submitting" @click="handleIndexSubmit">{{ t('common.save') }}</NButton>
            </template>
        </NModal>
    </div>
</template>

<style scoped>
.table-structure {
    height: 100%;
}

/* 强制 Tabs 使用 Flex 布局以撑满高度 */
:deep(.n-tabs) {
    display: flex;
    flex-direction: column;
    height: 100%;
}
:deep(.n-tabs-nav) {
    flex-shrink: 0;
}
:deep(.n-tabs-pane-wrapper) {
    flex: 1;
    min-height: 0;
    overflow: hidden;
}
:deep(.n-tab-pane) {
    height: 100%;
    padding: 0;
}

.pane-content {
    display: flex;
    flex-direction: column;
    height: 100%;
    padding-top: 10px;
}
.toolbar {
    margin-bottom: 8px;
    padding-right: 12px;
    flex-shrink: 0;
}
.table-container {
    flex: 1;
    min-height: 0;
    box-sizing: border-box;
}
</style>
