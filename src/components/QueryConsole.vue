<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import { 
  NCard, NInput, NButton, NSpace, NDataTable, 
  NIcon, useMessage, NAlert, NModal, NFormItem
} from 'naive-ui'
import { PlayOutline, SparklesOutline, SettingsOutline } from '@vicons/ionicons5'
import { invoke } from '../utils/tauri'
import { useI18n } from 'vue-i18n'
import type { ConnectionConfig } from '../types'
import AIConfigModal from './AIConfigModal.vue'

interface ColumnDef {
  name: string
  type_name: string
  is_pk: boolean
  is_nullable?: boolean
  default_value?: string
  comment?: string
}

const props = defineProps<{
  config: ConnectionConfig
  initialQuery?: string
  selectedTable?: string
  selectedDatabase?: string
}>()

const message = useMessage()
const { t } = useI18n()
const query = ref('')
const loading = ref(false)
const results = ref<any[]>([])
const error = ref('')
const executionTime = ref(0)
const lastQuery = ref('')

// AI related
const showAIModal = ref(false)
const showAIConfigModal = ref(false)
const aiPrompt = ref('')
const aiLoading = ref(false)

watch(() => props.initialQuery, (newVal) => {
  if (newVal) {
    query.value = newVal
  }
})

const columns = computed(() => {
  if (results.value.length === 0) return []
  const firstRow = results.value[0]
  return Object.keys(firstRow)
    .filter(key => key !== '__id')
    .map(key => ({
    title: key,
    key: key,
    width: 150,
    ellipsis: { tooltip: true },
    render(row: any) {
        const val = row[key];
        if (typeof val === 'object' && val !== null) {
            return JSON.stringify(val);
        }
        return val;
    }
  }))
})

async function runQuery() {
  if (!query.value.trim()) return
  
  loading.value = true
  error.value = ''
  results.value = []
  const start = performance.now()
  
  try {
    const data = await invoke<any[]>('execute_query', { 
      config: props.config, 
      query: query.value 
    })
    results.value = data.map((item: any, index: number) => ({ ...item, __id: index }))
    lastQuery.value = query.value
    executionTime.value = Math.round(performance.now() - start)
    message.success(t('manage.query_success', { time: executionTime.value, rows: data.length }))
  } catch (err: any) {
    error.value = err.toString()
  } finally {
    loading.value = false
  }
}

function openAIModal() {
  aiPrompt.value = ''
  showAIModal.value = true
}

async function generateSQL() {
  if (!aiPrompt.value.trim()) {
    message.warning(t('ai.enter_prompt'))
    return
  }
  
  aiLoading.value = true
  try {
    // Get table schema for the current table
    let tableSchemas = ''
    if (props.selectedTable) {
      try {
        const columns = await invoke<ColumnDef[]>('get_columns', {
          config: props.config,
          table: props.selectedTable,
          database: props.selectedDatabase
        })
        tableSchemas = `表名: ${props.selectedTable}\n字段:\n` + 
          columns.map(c => `  - ${c.name} (${c.type_name})${c.is_pk ? ' [主键]' : ''}${c.is_nullable === false ? ' [非空]' : ''}`).join('\n')
      } catch (e) {
        console.error('Failed to get columns:', e)
      }
    }
    
    if (!tableSchemas) {
      tableSchemas = '(未选择表，请根据常见数据库结构生成通用查询)'
    }
    
    const sql = await invoke<string>('generate_sql_from_text', {
      dbType: props.config.db_type,
      tableSchemas: tableSchemas,
      userRequest: aiPrompt.value
    })
    
    query.value = sql
    showAIModal.value = false
    message.success(t('ai.sql_generated'))
  } catch (err: any) {
    message.error(err.toString())
  } finally {
    aiLoading.value = false
  }
}

// Expose run function if parent wants to trigger it
defineExpose({
  setQuery: (q: string) => { query.value = q },
  run: runQuery
})
</script>

<template>
  <div class="query-console">
    <NSpace vertical :size="12" style="height: 100%">
      <div class="editor-area">
        <NInput
            v-model:value="query"
            type="textarea"
            :placeholder="t('manage.query_placeholder')"
            :autosize="{ minRows: 4, maxRows: 8 }"
            style="font-family: monospace;"
        />
        <div class="actions">
             <NSpace>
               <NButton size="small" secondary @click="showAIConfigModal = true">
                  <template #icon><NIcon><SettingsOutline /></NIcon></template>
               </NButton>
               <NButton size="small" type="warning" @click="openAIModal">
                  <template #icon><NIcon><SparklesOutline /></NIcon></template>
                  {{ t('ai.generate_sql') }}
               </NButton>
               <NButton type="primary" size="small" :loading="loading" @click="runQuery">
                  <template #icon><NIcon><PlayOutline /></NIcon></template>
                  {{ t('manage.execute') }}
               </NButton>
             </NSpace>
        </div>
      </div>

      <div class="results-area">
         <NAlert v-if="error" type="error" :title="t('manage.execution_error')" closable class="error-alert">
            {{ error }}
         </NAlert>
         
         <NCard content-style="padding: 0; display: flex; flex-direction: column; height: 100%;" class="result-card">
            <NDataTable
                v-if="results.length > 0"
                :columns="columns"
                :data="results"
                :row-key="(row: any) => row.__id"
                flex-height
                :bordered="false"
                size="small"
                style="height: 100%"
            />
            <div v-else-if="!loading && !error && lastQuery" class="no-data">
               {{ t('manage.no_data_returned') }}
            </div>
             <div v-else-if="!lastQuery" class="no-data">
               {{ t('manage.ready_to_execute') }}
            </div>
         </NCard>
      </div>
    </NSpace>
    
    <!-- AI Generate SQL Modal -->
    <NModal v-model:show="showAIModal" preset="card" :title="t('ai.generate_sql')" style="width: 500px;">
      <NFormItem :label="t('ai.describe_query')">
        <NInput 
          v-model:value="aiPrompt" 
          type="textarea" 
          :placeholder="t('ai.prompt_placeholder')"
          :autosize="{ minRows: 3, maxRows: 6 }"
        />
      </NFormItem>
      <div v-if="props.selectedTable" class="current-table">
        {{ t('ai.current_table') }}: <strong>{{ props.selectedTable }}</strong>
      </div>
      <template #footer>
        <NSpace justify="end">
          <NButton @click="showAIModal = false">{{ t('common.cancel') }}</NButton>
          <NButton type="primary" @click="generateSQL" :loading="aiLoading">
            <template #icon><NIcon><SparklesOutline /></NIcon></template>
            {{ t('ai.generate') }}
          </NButton>
        </NSpace>
      </template>
    </NModal>
    
    <!-- AI Config Modal -->
    <AIConfigModal v-model:show="showAIConfigModal" @saved="message.success(t('common.success'))" />
  </div>
</template>

<style scoped>
.query-console {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.editor-area {
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: 100%;
}
.actions {
  display: flex;
  justify-content: flex-end;
  flex-wrap: wrap;
  gap: 8px;
  padding-right: 4px; /* Small safe area */
  width: 100%;
}
.results-area {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
  gap: 12px;
}
.result-card {
    flex: 1;
    min-height: 0;
}
.no-data {
    display: flex;
    justify-content: center;
    align-items: center;
    height: 100%;
    color: var(--n-text-color-3);
    font-style: italic;
}
.error-alert {
    flex-shrink: 0;
}
.current-table {
  font-size: 12px;
  color: var(--n-text-color-3);
  margin-top: 8px;
}
</style>

