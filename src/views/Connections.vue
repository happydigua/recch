<script setup lang="ts">
import { ref, onMounted, h, computed } from 'vue'
import { useRouter } from 'vue-router'
import {
  NCard,
  NButton,
  NSpace,
  NEmpty,
  NIcon,
  NDataTable,
  NH2,
  useMessage,
  useDialog,
  type DataTableColumns
} from 'naive-ui'
import { AddOutline, ServerOutline, CreateOutline, TrashOutline } from '@vicons/ionicons5'
import { invoke } from '../utils/tauri'
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
  { title: t('connection.port'), key: 'port' },
  {
    title: t('common.edit'), // Reuse common keys or add specific 'Actions' key
    key: 'actions',
    render(row) {
      return h(NSpace, {}, {
        default: () => [
          h(
            NButton,
            {
              size: 'small',
              onClick: () => handleConnect(row)
            },
            { default: () => t('connection.connect') }
          ),
          h(
            NButton,
            {
              size: 'small',
              secondary: true,
              onClick: () => handleEdit(row)
            },
            { icon: () => h(NIcon, null, { default: () => h(CreateOutline) }) }
          ),
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
        ]
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

onMounted(() => {
  loadConnections()
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
  </div>
</template>

<style scoped>
.connections {
  max-width: 1000px;
}
</style>
