<script setup lang="ts">
import { ref, watch } from 'vue'
import { 
  NCard, NSpace, NTag, NCode, NSpin, NEmpty, NDescriptions, NDescriptionsItem,
  NIcon, NButton
} from 'naive-ui'
import { RefreshOutline, TimeOutline } from '@vicons/ionicons5'
import { invoke } from '../utils/tauri'
import type { ConnectionConfig } from '../types'

interface RedisKeyInfo {
  key: string
  key_type: string
  ttl: number
  value: string
  length?: number
}

const props = defineProps<{
  config: ConnectionConfig
  selectedKey: string
  database?: string
}>()

const loading = ref(false)
const keyInfo = ref<RedisKeyInfo | null>(null)
const error = ref('')

async function loadKeyInfo() {
  if (!props.selectedKey) return
  
  loading.value = true
  error.value = ''
  try {
    const info = await invoke<RedisKeyInfo>('get_redis_key_value', {
      config: props.config,
      key: props.selectedKey,
      database: props.database
    })
    keyInfo.value = info
  } catch (e: any) {
    error.value = e.toString()
  } finally {
    loading.value = false
  }
}

watch(() => props.selectedKey, () => {
  loadKeyInfo()
}, { immediate: true })

function getTypeColor(type: string): 'default' | 'info' | 'warning' | 'error' | 'success' | 'primary' {
  const colors: Record<string, 'default' | 'info' | 'warning' | 'error' | 'success' | 'primary'> = {
    'string': 'success',
    'list': 'info',
    'set': 'warning',
    'zset': 'error',
    'hash': 'default'
  }
  return colors[type] || 'default'
}

function formatTTL(ttl: number): string {
  if (ttl === -1) return '永不过期'
  if (ttl === -2) return 'Key 不存在'
  if (ttl < 60) return `${ttl} 秒`
  if (ttl < 3600) return `${Math.floor(ttl / 60)} 分钟`
  if (ttl < 86400) return `${Math.floor(ttl / 3600)} 小时`
  return `${Math.floor(ttl / 86400)} 天`
}
</script>

<template>
  <div class="redis-viewer">
    <NSpin :show="loading">
      <NEmpty v-if="!selectedKey" description="选择一个 Key 查看详情" />
      
      <div v-else-if="keyInfo" class="key-details">
        <NCard size="small" class="info-card">
          <template #header>
            <NSpace align="center" justify="space-between">
              <span class="key-name">{{ keyInfo.key }}</span>
              <NSpace>
                <NTag :type="getTypeColor(keyInfo.key_type)" size="small">
                  {{ keyInfo.key_type.toUpperCase() }}
                </NTag>
                <NButton text size="tiny" @click="loadKeyInfo">
                  <template #icon><NIcon><RefreshOutline /></NIcon></template>
                </NButton>
              </NSpace>
            </NSpace>
          </template>
          
          <NDescriptions :column="2" label-placement="left" size="small">
            <NDescriptionsItem label="类型">
              <NTag :type="getTypeColor(keyInfo.key_type)" size="small">
                {{ keyInfo.key_type }}
              </NTag>
            </NDescriptionsItem>
            <NDescriptionsItem label="TTL">
              <NSpace align="center" :size="4">
                <NIcon><TimeOutline /></NIcon>
                {{ formatTTL(keyInfo.ttl) }}
              </NSpace>
            </NDescriptionsItem>
            <NDescriptionsItem v-if="keyInfo.length !== undefined" label="长度">
              {{ keyInfo.length }} 个元素
            </NDescriptionsItem>
          </NDescriptions>
        </NCard>
        
        <NCard size="small" title="值" class="value-card">
          <NCode :code="keyInfo.value" language="json" word-wrap />
        </NCard>
      </div>
      
      <div v-else-if="error" class="error-state">
        {{ error }}
      </div>
    </NSpin>
  </div>
</template>

<style scoped>
.redis-viewer {
  height: 100%;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding-right: 12px;
}

.key-details {
  display: flex;
  flex-direction: column;
  gap: 12px;
  height: 100%;
}

.key-name {
  font-family: monospace;
  font-weight: bold;
  font-size: 14px;
}

.info-card {
  flex-shrink: 0;
}

.value-card {
  flex: 1;
  min-height: 200px;
  overflow: auto;
}

.error-state {
  color: var(--n-error-color);
  padding: 20px;
  text-align: center;
}
</style>
