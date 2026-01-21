<script setup lang="ts">
import { ref, watch } from 'vue'
import { 
  NModal, NForm, NFormItem, NInput, NInputNumber, 
  NSelect, NButton, NSpace, useMessage 
} from 'naive-ui'
import { invoke, isTauri } from '../utils/tauri'
import { v4 as uuidv4 } from 'uuid'
import { useI18n } from 'vue-i18n'
import type { ConnectionConfig } from '../types'

const props = defineProps<{
  show: boolean
  connection?: ConnectionConfig | null
}>()

const emit = defineEmits<{
  (e: 'update:show', value: boolean): void
  (e: 'saved'): void
}>()

const message = useMessage()
const { t } = useI18n()
const loading = ref(false)
const testing = ref(false)

const formModel = ref<ConnectionConfig>({
  id: '',
  name: '',
  db_type: 'mysql',
  host: 'localhost',
  port: 3306,
  username: 'root',
  password: '',
  database: ''
})

const dbTypeOptions = [
  { label: 'MySQL', value: 'mysql' },
  { label: 'PostgreSQL', value: 'postgresql' },
  { label: 'Redis', value: 'redis' }
]

// Watch for changes in db_type to set default port
watch(() => formModel.value.db_type, (newType) => {
  if (newType === 'mysql') formModel.value.port = 3306
  if (newType === 'postgresql') formModel.value.port = 5432
  if (newType === 'redis') formModel.value.port = 6379
})

// Watch for editing connection
watch(() => props.connection, (newVal) => {
  if (newVal) {
    formModel.value = { ...newVal }
  } else {
    // Reset form
    formModel.value = {
      id: '',
      name: '',
      db_type: 'mysql',
      host: 'localhost',
      port: 3306,
      username: 'root',
      password: '',
      database: ''
    }
  }
}, { immediate: true })

async function handleTest() {
  if (!isTauri()) {
    message.error('未检测到 Tauri 环境，请确保是在桌面原生窗口中运行')
    return
  }
  testing.value = true
  try {
    // Ensure ID exists for the test object (though backend might not check it for test)
    if (!formModel.value.id) formModel.value.id = uuidv4()
    
    const result = await invoke('test_connection', { config: formModel.value })
    // Backend returns raw string, maybe we can map it to i18n if it's standard, 
    // but for now let's just show what backend says or a success message
    message.success(t('connection.test_success') + `: ${result}`)
  } catch (error) {
    message.error(t('connection.test_failed') + ': ' + error)
  } finally {
    testing.value = false
  }
}

async function handleSave() {
  if (!isTauri()) {
    message.error('未检测到 Tauri 环境，无法保存配置')
    return
  }
  loading.value = true
  try {
    if (!formModel.value.id) formModel.value.id = uuidv4()
    
    await invoke('save_connection', { config: formModel.value })
    message.success(t('common.success'))
    emit('saved')
    emit('update:show', false)
  } catch (error) {
    message.error(t('common.error') + ': ' + error)
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <NModal
    :show="show"
    @update:show="emit('update:show', $event)"
    preset="card"
    :title="connection ? t('connection.edit') : t('connection.new')"
    style="width: 600px"
  >
    <NForm :model="formModel" label-placement="left" label-width="100">
      <NFormItem :label="t('connection.name')" path="name">
        <NInput v-model:value="formModel.name" :placeholder="t('connection.name_placeholder')" />
      </NFormItem>
      
      <NFormItem :label="t('connection.db_type')" path="db_type">
        <NSelect v-model:value="formModel.db_type" :options="dbTypeOptions" />
      </NFormItem>

      <NFormItem :label="t('connection.host')" path="host">
        <NInput v-model:value="formModel.host" placeholder="localhost" />
      </NFormItem>

      <NFormItem :label="t('connection.port')" path="port">
        <NInputNumber v-model:value="formModel.port" style="width: 100%" :show-button="false" />
      </NFormItem>

      <template v-if="formModel.db_type !== 'redis'">
        <NFormItem :label="t('connection.username')" path="username">
          <NInput v-model:value="formModel.username" placeholder="root" />
        </NFormItem>
      </template>

      <NFormItem :label="t('connection.password')" path="password">
        <NInput
          v-model:value="formModel.password"
          type="password"
          show-password-on="click"
          placeholder=""
        />
      </NFormItem>
      
      <NFormItem :label="t('connection.database')" path="database">
        <NInput v-model:value="formModel.database" :placeholder="t('connection.database_placeholder')" />
      </NFormItem>
    </NForm>

    <template #footer>
      <NSpace justify="end">
        <NButton @click="handleTest" :loading="testing" secondary type="warning">
          {{ t('connection.test') }}
        </NButton>
        <NButton @click="handleSave" :loading="loading" type="primary">
          {{ t('common.save') }}
        </NButton>
      </NSpace>
    </template>
  </NModal>
</template>
