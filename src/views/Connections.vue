<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, h, computed } from 'vue'
import { useRouter } from 'vue-router'
import {
  NCard,
  NButton,
  NSpace,
  NEmpty,
  NIcon,
  NDataTable,
  NH2,
  NModal,
  NProgress,
  NSelect,
  NSpin,
  useMessage,
  useDialog,
  type DataTableColumns
} from 'naive-ui'
import { AddOutline, ServerOutline, CreateOutline, TrashOutline, DownloadOutline, CloudUploadOutline } from '@vicons/ionicons5'
import { invoke, isTauri } from '../utils/tauri'
import { save, open } from '@tauri-apps/plugin-dialog'
import { readTextFile } from '@tauri-apps/plugin-fs'
import ConnectionModal from '../components/ConnectionModal.vue'
import { useI18n } from 'vue-i18n'
import type { ConnectionConfig } from '../types'

const message = useMessage()
const dialog = useDialog()
const { t } = useI18n()

const connections = ref<ConnectionConfig[]>([])
const showModal = ref(false)
const currentConnection = ref<ConnectionConfig | null>(null)
const loading = ref(false)
const showDatabaseModal = ref(false)
const databaseOptions = ref<{ label: string, value: string }[]>([])
const selectedDatabase = ref<string | null>(null)
const pendingDatabaseAction = ref<'export' | 'import' | null>(null)
const pendingConnection = ref<ConnectionConfig | null>(null)
const actionLoadingKey = ref<string | null>(null)
const progressVisible = ref(false)
const progressTitle = ref('')
const progressDescription = ref('')
const progressMode = ref<'export' | 'import' | null>(null)
const progressPercent = ref(0)
const progressMeta = ref('')
const currentExportTaskId = ref<string | null>(null)
const cancelExportLoading = ref(false)

let unlistenExportProgress: null | (() => void) = null

interface ExportProgressPayload {
  task_id: string
  database: string
  progress: number
  status: 'running' | 'completed' | 'cancelled' | 'error'
  stage: 'preparing' | 'schema' | 'counting' | 'fetching' | 'data' | 'table_complete' | 'completed' | 'cancelled' | 'error'
  table_name?: string | null
  processed_tables: number
  total_tables: number
  processed_rows: number
  table_rows: number
  error?: string | null
}

const columns = computed<DataTableColumns<ConnectionConfig>>(() => [
  { title: t('connection.name'), key: 'name' },
  { 
    title: t('connection.db_type'), 
    key: 'db_type',
    render(row) {
      const map: Record<string, string> = {
        mysql: 'MySQL',
        postgresql: 'PostgreSQL',
        redis: 'Redis'
      }
      return map[row.db_type] || row.db_type
    }
  },
  { title: t('connection.host'), key: 'host' },
  { title: t('connection.port'), key: 'port', width: 96 },
  {
    title: t('common.edit'), // Reuse common keys or add specific 'Actions' key
    key: 'actions',
    width: 420,
    render(row) {
      const exportLoading = actionLoadingKey.value === `${row.id}:export`
      const importLoading = actionLoadingKey.value === `${row.id}:import`
      const actionButtons = [
        h(
          NButton,
          {
            size: 'small',
            onClick: () => handleConnect(row)
          },
          { default: () => t('connection.connect') }
        )
      ]

      if (row.db_type !== 'redis') {
        actionButtons.push(
          h(
            NButton,
            {
              size: 'small',
              secondary: true,
              loading: exportLoading,
              onClick: () => handleDatabaseAction(row, 'export')
            },
            {
              icon: () => h(NIcon, null, { default: () => h(DownloadOutline) }),
              default: () => t('manage.export')
            }
          )
        )
        actionButtons.push(
          h(
            NButton,
            {
              size: 'small',
              secondary: true,
              loading: importLoading,
              onClick: () => handleDatabaseAction(row, 'import')
            },
            {
              icon: () => h(NIcon, null, { default: () => h(CloudUploadOutline) }),
              default: () => t('manage.import')
            }
          )
        )
      }

      actionButtons.push(
        h(
          NButton,
          {
            size: 'small',
            secondary: true,
            onClick: () => handleEdit(row)
          },
          { icon: () => h(NIcon, null, { default: () => h(CreateOutline) }) }
        )
      )
      actionButtons.push(
        h(
          NButton,
          {
            size: 'small',
            type: 'error',
            secondary: true,
            onClick: () => handleDelete(row)
          },
          { icon: () => h(NIcon, null, { default: () => h(TrashOutline) }) }
        )
      )

      return h(NSpace, {}, {
        default: () => actionButtons
      })
    }
  }
])

async function loadConnections() {
  loading.value = true
  try {
    connections.value = await invoke('get_connections')
  } catch (error) {
    message.error(t('common.error') + ': ' + error)
  } finally {
    loading.value = false
  }
}

function handleAdd() {
  currentConnection.value = null
  showModal.value = true
}

function handleEdit(row: ConnectionConfig) {
  currentConnection.value = row
  showModal.value = true
}

function handleDelete(row: ConnectionConfig) {
  dialog.warning({
    title: t('common.delete'),
    content: t('common.confirm_delete'),
    positiveText: t('common.delete'),
    negativeText: t('common.cancel'),
    onPositiveClick: async () => {
      try {
        await invoke('delete_connection', { id: row.id })
        message.success(t('common.success'))
        loadConnections()
      } catch (error) {
        message.error(t('common.error') + ': ' + error)
      }
    }
  })
}

const router = useRouter()

function handleConnect(row: ConnectionConfig) {
  message.info(t('common.loading'))
  router.push(`/manage/${row.id}`)
}

function resetDatabaseModal() {
  showDatabaseModal.value = false
  databaseOptions.value = []
  selectedDatabase.value = null
  pendingDatabaseAction.value = null
  pendingConnection.value = null
}

function showProgress(title: string, description: string, mode: 'export' | 'import') {
  progressTitle.value = title
  progressDescription.value = description
  progressMode.value = mode
  progressPercent.value = 0
  progressMeta.value = ''
  cancelExportLoading.value = false
  progressVisible.value = true
}

function hideProgress() {
  progressVisible.value = false
  progressTitle.value = ''
  progressDescription.value = ''
  progressMode.value = null
  progressPercent.value = 0
  progressMeta.value = ''
  currentExportTaskId.value = null
  cancelExportLoading.value = false
}

function createExportTaskId() {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  return `export-${Date.now()}`
}

function formatExportProgress(payload: ExportProgressPayload) {
  const table = payload.table_name || ''

  switch (payload.stage) {
    case 'schema':
      return t('manage.export_progress_schema', { table })
    case 'counting':
      return t('manage.export_progress_counting', { table })
    case 'fetching':
      return t('manage.export_progress_fetching', { table })
    case 'data':
      return t('manage.export_progress_data', {
        table,
        current: payload.processed_rows,
        total: payload.table_rows
      })
    case 'table_complete':
      return t('manage.export_progress_table_complete', { table })
    case 'cancelled':
      return t('manage.stopping_export')
    case 'error':
      return payload.error || t('manage.database_export_failed')
    case 'completed':
      return t('manage.database_export_success', { name: payload.database })
    case 'preparing':
    default:
      return t('manage.export_progress_preparing')
  }
}

function formatExportMeta(payload: ExportProgressPayload) {
  if (payload.total_tables <= 0) {
    return ''
  }

  const tablesText = t('manage.export_progress_tables', {
    current: payload.processed_tables,
    total: payload.total_tables
  })

  if (!payload.table_name || payload.table_rows <= 0) {
    return tablesText
  }

  const rowsText = t('manage.export_progress_rows', {
    current: payload.processed_rows,
    total: payload.table_rows
  })

  return `${tablesText} · ${rowsText}`
}

async function setupExportProgressListener() {
  if (!isTauri()) return

  const { listen } = await import('@tauri-apps/api/event')
  unlistenExportProgress = await listen<ExportProgressPayload>('database-export-progress', (event) => {
    const payload = event.payload
    if (!payload || payload.task_id !== currentExportTaskId.value) {
      return
    }

    progressPercent.value = payload.progress
    progressDescription.value = formatExportProgress(payload)
    progressMeta.value = formatExportMeta(payload)

    if (payload.status !== 'running') {
      cancelExportLoading.value = false
    }
  })
}

async function stopExport() {
  const taskId = currentExportTaskId.value
  if (!taskId || cancelExportLoading.value) return

  cancelExportLoading.value = true
  progressDescription.value = t('manage.stopping_export')

  try {
    await invoke('cancel_database_export', { taskId })
  } catch (error) {
    cancelExportLoading.value = false
    if (!`${error}`.includes('Export task not found')) {
      message.error(t('manage.database_export_failed') + ': ' + error)
    }
  }
}

async function exportDatabase(row: ConnectionConfig, database: string) {
  try {
    const filePath = await save({
      defaultPath: `${database}.sql`,
      filters: [{ name: 'SQL', extensions: ['sql'] }]
    })
    if (!filePath) return

    const taskId = createExportTaskId()
    currentExportTaskId.value = taskId
    showProgress(
      t('manage.export'),
      t('manage.exporting_database', { name: database }),
      'export'
    )
    await invoke('export_database_sql', {
      config: row,
      database,
      taskId,
      outputPath: filePath
    })
    message.success(t('manage.database_export_success', { name: database }))
  } catch (error) {
    if (`${error}`.includes('Export cancelled')) {
      message.info(t('manage.database_export_cancelled', { name: database }))
    } else {
      message.error(t('manage.database_export_failed') + ': ' + error)
    }
  } finally {
    hideProgress()
  }
}

async function importDatabase(row: ConnectionConfig, database: string) {
  try {
    const filePath = await open({
      filters: [{ name: 'SQL', extensions: ['sql'] }],
      multiple: false
    })
    if (!filePath) return

    dialog.warning({
      title: t('manage.import'),
      content: t('manage.database_import_confirm', { name: database }),
      positiveText: t('manage.import'),
      negativeText: t('common.cancel'),
      onPositiveClick: async () => {
        try {
          showProgress(
            t('manage.import'),
            t('manage.importing_database', { name: database }),
            'import'
          )
          const text = await readTextFile(filePath as string)
          await invoke('import_database_sql', { config: row, database, script: text })
          message.success(t('manage.database_import_success', { name: database }))
        } catch (error) {
          message.error(t('manage.database_import_failed') + ': ' + error)
        } finally {
          hideProgress()
        }
      }
    })
  } catch (error) {
    message.error(t('manage.database_import_failed') + ': ' + error)
  }
}

async function runDatabaseAction(row: ConnectionConfig, database: string, action: 'export' | 'import') {
  actionLoadingKey.value = `${row.id}:${action}`
  try {
    if (action === 'export') {
      await exportDatabase(row, database)
    } else {
      await importDatabase(row, database)
    }
  } finally {
    actionLoadingKey.value = null
  }
}

async function handleDatabaseAction(row: ConnectionConfig, action: 'export' | 'import') {
  if (row.database) {
    await runDatabaseAction(row, row.database, action)
    return
  }

  try {
    const dbs = await invoke<string[]>('get_databases', { config: row })
    if (!dbs.length) {
      message.warning(t('connection.no_databases'))
      return
    }

    if (dbs.length === 1) {
      await runDatabaseAction(row, dbs[0]!, action)
      return
    }

    pendingConnection.value = row
    pendingDatabaseAction.value = action
    databaseOptions.value = dbs.map(db => ({ label: db, value: db }))
    selectedDatabase.value = dbs[0] || null
    showDatabaseModal.value = true
  } catch (error) {
    message.error(t('common.error') + ': ' + error)
  }
}

async function confirmDatabaseAction() {
  if (!pendingConnection.value || !pendingDatabaseAction.value || !selectedDatabase.value) {
    message.warning(t('manage.no_database_selected'))
    return
  }

  const row = pendingConnection.value
  const action = pendingDatabaseAction.value
  const database = selectedDatabase.value
  resetDatabaseModal()
  await runDatabaseAction(row, database, action)
}

onMounted(async () => {
  loadConnections()
  await setupExportProgressListener()
})

onBeforeUnmount(() => {
  unlistenExportProgress?.()
})
</script>

<template>
  <div class="connections">
    <NSpace justify="space-between" align="center" style="margin-bottom: 24px;">
      <NH2 style="margin: 0;">{{ t('menu.connections') }}</NH2>
      <NButton type="primary" @click="handleAdd">
        <template #icon>
          <NIcon><AddOutline /></NIcon>
        </template>
        {{ t('connection.new') }}
      </NButton>
    </NSpace>

    <NCard>
      <NEmpty v-if="connections.length === 0 && !loading" :description="t('manage.no_tables')"> <!-- Reusing 'no tables' or creating 'no_connections' -->
        <template #icon>
          <NIcon size="64" color="rgba(255,255,255,0.3)">
            <ServerOutline />
          </NIcon>
        </template>
        <template #extra>
          <NButton type="primary" size="small" @click="handleAdd">
            {{ t('connection.new') }}
          </NButton>
        </template>
      </NEmpty>
      <NDataTable
        v-else
        :columns="columns"
        :data="connections"
        :bordered="false"
        :loading="loading"
      />
    </NCard>

    <ConnectionModal
      v-model:show="showModal"
      :connection="currentConnection"
      @saved="loadConnections"
    />

    <NModal v-model:show="showDatabaseModal" preset="dialog" :title="pendingDatabaseAction === 'export' ? t('manage.export') : t('manage.import')">
      <NSelect
        v-model:value="selectedDatabase"
        :options="databaseOptions"
        :placeholder="t('connection.database')"
      />
      <template #action>
        <NButton @click="resetDatabaseModal">{{ t('common.cancel') }}</NButton>
        <NButton type="primary" @click="confirmDatabaseAction">
          {{ pendingDatabaseAction === 'export' ? t('manage.export') : t('manage.import') }}
        </NButton>
      </template>
    </NModal>

    <NModal
      :show="progressVisible"
      preset="card"
      :title="progressTitle"
      :mask-closable="false"
      :closable="false"
      style="width: 360px;"
    >
      <div v-if="progressMode === 'export'" class="progress-panel">
        <div class="progress-text">{{ progressDescription }}</div>
        <NProgress
          type="line"
          :percentage="progressPercent"
          :processing="progressPercent < 100"
          :show-indicator="true"
        />
        <div v-if="progressMeta" class="progress-meta">{{ progressMeta }}</div>
        <div class="progress-actions">
          <NButton
            secondary
            type="warning"
            :loading="cancelExportLoading"
            :disabled="cancelExportLoading"
            @click="stopExport"
          >
            {{ t('manage.stop') }}
          </NButton>
        </div>
      </div>
      <div v-else class="progress-body">
        <NSpin size="small" />
        <div class="progress-text">{{ progressDescription }}</div>
      </div>
    </NModal>
  </div>
</template>

<style scoped>
.connections {
  max-width: 1000px;
}

.progress-body {
  display: flex;
  align-items: center;
  gap: 12px;
  min-height: 56px;
}

.progress-panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.progress-text {
  line-height: 1.5;
}

.progress-meta {
  color: rgba(0, 0, 0, 0.55);
  font-size: 12px;
}

.progress-actions {
  display: flex;
  justify-content: flex-end;
}
</style>
