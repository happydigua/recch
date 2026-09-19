<script setup lang="ts">
import { ref, watch, computed, onBeforeUnmount } from 'vue'
import { 
  NCard, NInput, NButton, NSpace, NDataTable, 
  NIcon, useMessage, NAlert, NModal, NFormItem, NCheckbox
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
const aiConsent = ref(false)
const aiEndpoint = ref('')
let epoch = 0
let disposed = false
onBeforeUnmount(() => { disposed = true; epoch++ })
watch(() => JSON.stringify([props.config, props.selectedDatabase, props.selectedTable]), () => {
  epoch++
  results.value = []; error.value = ''; lastQuery.value = ''
  loading.value = false; aiLoading.value = false
  aiConsent.value = false; showAIModal.value = false
}, { flush: 'sync' })
const isCurrent = (ticket: number) => !disposed && ticket === epoch


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
  if (!query.value.trim() || loading.value) return
  const ticket = epoch
  const sql = query.value
  const config = { ...props.config, database: props.selectedDatabase ?? props.config.database }
  loading.value = true; error.value = ''; results.value = []
  const start = performance.now()
  try {
    const data = await invoke<any[]>('execute_query', { config, query: sql })
    if (!isCurrent(ticket)) return
    results.value = data.map((item, index) => ({ ...item, __id: index }))
    lastQuery.value = sql
    executionTime.value = Math.round(performance.now() - start)
    if (config.db_type === 'redis' && data.some(item => item.error)) {
      error.value = 'Redis 批次包含失败命令；请检查结果。之前成功执行的命令不会自动撤销。'
    } else message.success(t('manage.query_success', { time: executionTime.value, rows: data.length }))
  } catch (err) {
    if (isCurrent(ticket)) error.value = String(err)
  } finally {
    if (isCurrent(ticket)) loading.value = false
  }
}

async function openAIModal() {
  const ticket = epoch
  aiPrompt.value = ''; aiConsent.value = false; aiEndpoint.value = ''
  try {
    const settings = await invoke<{ api_url: string }>('get_ai_config')
    if (!isCurrent(ticket)) return
    aiEndpoint.value = settings.api_url
    showAIModal.value = true
  } catch (err) { if (isCurrent(ticket)) message.error(String(err)) }
}

async function generateSQL() {
  if (aiLoading.value || !aiConsent.value) return
  if (!aiPrompt.value.trim()) { message.warning(t('ai.enter_prompt')); return }
  const ticket = epoch
  const config = { ...props.config }
  const table = props.selectedTable
  const database = props.selectedDatabase
  const prompt = aiPrompt.value
  const expectedApiUrl = aiEndpoint.value
  aiLoading.value = true
  try {
    let tableSchemas = '(未选择表，请根据常见数据库结构生成通用查询)'
    if (table) {
      const columns = await invoke<ColumnDef[]>('get_columns', { config, table, database })
      if (!isCurrent(ticket)) return
      tableSchemas = `表名: ${table}\n字段:\n` + columns.map(c => `  - ${c.name} (${c.type_name})${c.is_pk ? ' [主键]' : ''}`).join('\n')
    }
    const sql = await invoke<string>('generate_sql_from_text', {
      dbType: config.db_type, tableSchemas, userRequest: prompt, consent: true, expectedApiUrl
    })
    if (!isCurrent(ticket)) return
    query.value = sql; showAIModal.value = false
    message.success(t('ai.sql_generated'))
  } catch (err) { if (isCurrent(ticket)) message.error(String(err)) }
  finally { if (isCurrent(ticket)) aiLoading.value = false }
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
            :disabled="aiLoading"
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
      <NAlert type="warning" style="margin-bottom: 12px;">
        AI 请求会把提示词、表名和字段结构发送到：{{ aiEndpoint || '默认通义千问服务' }}。不要填写敏感业务数据。
        生成的语句可能修改或删除数据，请先审阅；不会自动执行。
      </NAlert>
      <NCheckbox v-model:checked="aiConsent" style="margin-bottom: 12px;">同意将上述内容发送到该 AI 服务</NCheckbox>
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
          <NButton type="primary" @click="generateSQL" :loading="aiLoading" :disabled="!aiConsent">
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
  padding-right: 12px; /* Safe area for buttons */
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

