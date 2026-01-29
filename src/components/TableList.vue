<script setup lang="ts">
import { ref, onMounted, computed, h } from 'vue'
import { NInput, NSpace, NSpin, NEmpty, NIcon, NButton, NTree, type TreeOption } from 'naive-ui'
import { SearchOutline, RefreshOutline, FlashOutline, FolderOutline, KeyOutline } from '@vicons/ionicons5'
import { invoke } from '../utils/tauri'
import { useI18n } from 'vue-i18n'
import type { ConnectionConfig } from '../types'

interface TableInfo {
  name: string
  data_size?: number
  index_size?: number
  total_size?: number
  row_count?: number
  comment?: string
}

function formatBytes(bytes: number | undefined): string {
  if (bytes === undefined || bytes === null) return ''
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i]
}

function formatNumber(num: number | undefined): string {
    if (num === undefined || num === null) return ''
    if (num >= 1000000) {
        return (num / 1000000).toFixed(1) + 'M'
    }
    if (num >= 1000) {
        return (num / 1000).toFixed(1) + 'k'
    }
    return num.toString()
}

function renderLabel(option: { name: string, row_count?: number, total_size?: number }) {
    return h('div', { 
        style: 'display: flex; align-items: center; width: 100%; overflow: hidden;' 
    }, [
        h('span', { 
            style: 'overflow: hidden; text-overflow: ellipsis; white-space: nowrap; margin-right: 8px;' 
        }, option.name),
        h('div', { style: 'flex: 1' }), // Spacer
        h('div', { 
            style: 'display: flex; gap: 8px; font-size: 11px; color: #999; flex-shrink: 0; align-items: center;' 
        }, [
            option.row_count !== undefined && option.row_count !== null 
                ? h('span', null, `${formatNumber(option.row_count)} rows`)
                : null,
             (option.row_count !== undefined && option.total_size !== undefined) ? h('span', {style: 'opacity: 0.3'}, '|') : null,
            option.total_size !== undefined && option.total_size !== null
                ? h('span', null, formatBytes(option.total_size))
                : null
        ].filter(Boolean))
    ])
}

const props = defineProps<{
  config: ConnectionConfig
}>()

const emit = defineEmits<{
  (e: 'select', data: { table: string, database?: string }): void
}>()

const { t } = useI18n()
const treeData = ref<TreeOption[]>([])
const loading = ref(false)
const searchText = ref('')

// Check if we are in single DB mode or Multi-DB mode
const isSingleDb = computed(() => !!props.config.database)
const isRedis = computed(() => props.config.db_type === 'redis')

async function loadRoot() {
  loading.value = true
  treeData.value = []
  try {
    if (isSingleDb.value) {
      const tables = await invoke<TableInfo[]>('get_tables', { config: props.config })
      treeData.value = tables.map(t => {
        return {
          label: () => renderLabel(t),
          key: t.name,
          type: 'table',
          isLeaf: true,
          prefix: () => h(NIcon, null, { default: () => h(isRedis.value ? KeyOutline : FlashOutline) })
        }
      })
    } else {
      // Multi DB: fetch databases
      const dbs = await invoke<string[]>('get_databases', { config: props.config })
      treeData.value = dbs.map(db => ({
        label: db,
        key: db,
        type: 'database',
        isLeaf: false,
        prefix: () => h(NIcon, null, { default: () => h(FolderOutline) })
        // Children will be loaded on expand
      }))
    }
  } catch (error) {
    console.error('Failed to load root', error)
  } finally {
    loading.value = false
  }
}

async function handleLoadChildren(node: TreeOption) {
    if (node.type === 'database') {
        const dbName = node.key as string
        try {
            const tables = await invoke<TableInfo[]>('get_tables', { 
                config: props.config,
                database: dbName
            })
            node.children = tables.map(t => {
                return {
                    label: () => renderLabel(t),
                    key: `${dbName}.${t.name}`, // Unique key
                    type: 'table',
                    isLeaf: true,
                    prefix: () => h(NIcon, null, { default: () => h(isRedis.value ? KeyOutline : FlashOutline) }),
                    uniqueKey: `${dbName}.${t.name}`,
                    dbName: dbName, // Custom payload
                    tableName: t.name
                }
            })
            return Promise.resolve()
        } catch (e) {
            console.error(e)
            return Promise.reject()
        }
    }
    return Promise.resolve()
}

function handleNodeClick(_keys: string[], option: (TreeOption | null)[]) {
    if (!option || option.length === 0) return
    const node = option[0]
    if (node && node.type === 'table') {
        if (isSingleDb.value) {
             emit('select', { table: node.key as string })
        } else {
             // For multi-db, key is db.table, but we stored metadata
             // Need to cast to any to access custom props or use typed custom option
             emit('select', { 
                 table: (node as any).tableName,
                 database: (node as any).dbName
             })
        }
    }
}

onMounted(() => {
  loadRoot()
})
</script>

<template>
  <div class="table-list">
    <NSpace vertical :size="12" style="height: 100%">
      <NSpace justify="space-between" align="center">
         <span class="title">{{ isRedis ? 'Keys' : (isSingleDb ? t('manage.tables') : t('connection.database')) }}</span>
         <NButton text size="tiny" @click="loadRoot">
            <template #icon><NIcon><RefreshOutline /></NIcon></template>
         </NButton>
      </NSpace>
      
      <div class="search-box">
          <NInput v-model:value="searchText" :placeholder="t('common.search')" size="small">
            <template #prefix>
              <NIcon><SearchOutline /></NIcon>
            </template>
          </NInput>
      </div>

      <div class="tree-container">
        <NSpin :show="loading">
             <NTree
                block-line
                expand-on-click
                :data="treeData"
                :pattern="searchText"
                :show-irrelevant-nodes="false"
                :on-load="handleLoadChildren"
                @update:selected-keys="handleNodeClick"
                virtual-scroll
                style="height: 100%"
             />
             <NEmpty v-if="!loading && treeData.length === 0" :description="t('manage.no_tables')" style="margin-top: 20px" />
        </NSpin>
      </div>
    </NSpace>
  </div>
</template>

<style scoped>
.table-list {
  height: 100%;
  display: flex;
  flex-direction: column;
}

/* Allow tree nodes to expand to full width for our flex layout */
/* Leveraging global or deep selector might be needed if scoped doesn't reach internal node */
:deep(.n-tree-node-content) {
    flex: 1;
    overflow: hidden;
}

.title {
  font-weight: bold;
  opacity: 0.8;
}

.search-box {
    flex-shrink: 0;
}

.tree-container {
  flex: 1;
  overflow: hidden; 
  min-height: 200px;
}
</style>
