<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { 
  NLayout, NLayoutSider, NLayoutContent, NTabs, NTabPane, 
  NResult, NButton, NSpin 
} from 'naive-ui'
import { invoke } from '../utils/tauri'
import { useI18n } from 'vue-i18n'
import type { ConnectionConfig } from '../types'
import TableList from '../components/TableList.vue'
import QueryConsole from '../components/QueryConsole.vue'
import DataGrid from '../components/DataGrid.vue'
import TableStructure from '../components/TableStructure.vue'
import RedisViewer from '../components/RedisViewer.vue'

const route = useRoute()
const router = useRouter()
const { t } = useI18n()
const connectionId = route.params.id as string

const loading = ref(true)
const config = ref<ConnectionConfig | null>(null)
const activeTab = ref('data')
const queryRef = ref<InstanceType<typeof QueryConsole> | null>(null)
const selectedTable = ref('')
const selectedDatabase = ref<string | undefined>(undefined)

const isRedis = computed(() => config.value?.db_type === 'redis')

async function loadConfig() {
  loading.value = true
  try {
    const connections = await invoke<ConnectionConfig[]>('get_connections')
    const found = connections.find(c => c.id === connectionId)
    if (found) {
      config.value = found
    } else {
      // Handle not found
    }
  } catch (e) {
    console.error(e)
  } finally {
    loading.value = false
  }
}

function handleTableSelect(data: { table: string, database?: string }) {
  selectedTable.value = data.table
  selectedDatabase.value = data.database
  
  if (data.database && config.value) {
      config.value = {
          ...config.value,
          database: data.database
      }
  }

  // Switch to data tab by default now
  activeTab.value = 'data'
  
  // Also pre-fill query console if needed
  if (queryRef.value) {
      const target = data.table // could be fully qualified
      queryRef.value.setQuery(`SELECT * FROM ${target} LIMIT 100;`)
  }
}

function goBack() {
  router.push('/connections')
}

onMounted(() => {
  loadConfig()
})
</script>

<template>
  <div class="manage-page">
    <div v-if="loading" class="loading-state">
      <NSpin size="large" :description="t('common.loading')" />
    </div>
    
    <NResult
      v-else-if="!config"
      status="404"
      title="Connection Not Found"
      description="The connection configuration could not be loaded."
    >
      <template #footer>
        <NButton @click="goBack">{{ t('manage.back') }}</NButton>
      </template>
    </NResult>

    <NLayout v-else has-sider class="layout">
      <NLayoutSider
        bordered
        width="240"
        content-style="padding: 12px; display: flex; flex-direction: column;"
      >
        <div class="sider-header">
           <NButton text @click="goBack" class="back-btn">← {{ t('manage.back') }}</NButton>
           <h3 class="conn-name">{{ config.name }}</h3>
        </div>
        <div class="sider-content">
            <TableList :config="config" @select="handleTableSelect" />
        </div>
      </NLayoutSider>
      
      <NLayoutContent content-style="padding: 12px 40px 12px 12px; height: 100%; overflow: hidden;">
         <div class="main-content">
            <NTabs v-model:value="activeTab" type="line" animated style="height: 100%; display: flex; flex-direction: column;">
                <!-- Redis-specific view -->
                <template v-if="isRedis">
                    <NTabPane name="data" tab="Key 详情" display-directive="show:lazy" style="height: 100%;">
                        <RedisViewer 
                            v-if="selectedTable"
                            :config="config" 
                            :selectedKey="selectedTable"
                            :database="selectedDatabase"
                        />
                        <div v-else class="no-selection">选择一个 Key 查看详情</div>
                    </NTabPane>
                    <NTabPane name="query" :tab="t('manage.query')" display-directive="show:lazy" style="height: 100%;">
                        <QueryConsole 
                            ref="queryRef" 
                            :config="config" 
                            :selectedTable="selectedTable"
                            :selectedDatabase="selectedDatabase"
                            style="height: 100%;" 
                        />
                    </NTabPane>
                </template>
                <!-- SQL Database view -->
                <template v-else>
                    <NTabPane name="data" :tab="t('manage.data')" display-directive="show:lazy" style="height: 100%;">
                        <DataGrid 
                            v-if="selectedTable"
                            :config="config" 
                            :table="selectedTable"
                            :database="selectedDatabase"
                        />
                        <div v-else class="no-selection">{{ t('manage.tables') }}</div>
                    </NTabPane>
                    <NTabPane name="structure" :tab="t('manage.structure')" display-directive="show:lazy" style="height: 100%;">
                        <TableStructure
                            v-if="selectedTable"
                            :config="config" 
                            :table="selectedTable"
                            :database="selectedDatabase"
                        />
                        <div v-else class="no-selection">{{ t('manage.tables') }}</div>
                    </NTabPane>
                    <NTabPane name="query" :tab="t('manage.query')" display-directive="show:lazy" style="height: 100%;">
                        <QueryConsole 
                            ref="queryRef" 
                            :config="config" 
                            :selectedTable="selectedTable"
                            :selectedDatabase="selectedDatabase"
                            style="height: 100%;" 
                        />
                    </NTabPane>
                </template>
                <NTabPane name="info" :tab="t('manage.info')">
                    <p>{{ t('manage.info') }}:</p>
                    <pre>{{ JSON.stringify(config, null, 2) }}</pre>
                </NTabPane>
            </NTabs>
         </div>
      </NLayoutContent>
    </NLayout>
  </div>
</template>

<style scoped>
.manage-page {
  height: 100vh;
  width: 100vw;
  /* Reset some default padding from layout if any */
}
.layout {
    height: 100%;
}
.loading-state {
  height: 100%;
  display: flex;
  justify-content: center;
  align-items: center;
}
.sider-header {
    margin-bottom: 12px;
}
.back-btn {
    margin-bottom: 8px;
    font-size: 0.9em;
    opacity: 0.7;
}
.conn-name {
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
}
.sider-content {
    flex: 1;
    min-height: 0;
}
.main-content {
    height: 100%;
}
.no-selection {
    display: flex;
    justify-content: center;
    align-items: center;
    height: 100%;
    color: var(--n-text-color-3);
    font-size: 1.1em;
}
:deep(.n-tabs-pane-wrapper) {
    flex: 1;
    min-height: 0; /* Important for nested scrolling */
    overflow-x: hidden; /* Prevent horizontal scroll causing cutoff */
    padding-right: 4px; /* Small safe buffer */
}
</style>
