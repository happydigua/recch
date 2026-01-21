<script setup lang="ts">
import { ref, watch } from 'vue'
import { 
  NModal, NForm, NFormItem, NInput, NSelect, NButton, NSpace, useMessage 
} from 'naive-ui'
import { invoke } from '../utils/tauri'
import { useI18n } from 'vue-i18n'

interface AIConfig {
  api_key: string
  api_url: string
  model: string
}

const props = defineProps<{
  show: boolean
}>()

const emit = defineEmits<{
  (e: 'update:show', value: boolean): void
  (e: 'saved'): void
}>()

const message = useMessage()
const { t } = useI18n()
const loading = ref(false)

const formModel = ref<AIConfig>({
  api_key: '',
  api_url: 'https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions',
  model: 'qwen-turbo'
})

// Common model presets - user can also type custom model names
const modelOptions = [
  { label: 'qwen-turbo', value: 'qwen-turbo' },
  { label: 'qwen-turbo-latest', value: 'qwen-turbo-latest' },
  { label: 'qwen-plus', value: 'qwen-plus' },
  { label: 'qwen-plus-latest', value: 'qwen-plus-latest' },
  { label: 'qwen-max', value: 'qwen-max' },
  { label: 'qwen-max-latest', value: 'qwen-max-latest' },
  { label: 'qwen-long', value: 'qwen-long' },
  { label: 'qwen-coder-plus', value: 'qwen-coder-plus' },
  { label: 'gpt-4o', value: 'gpt-4o' },
  { label: 'gpt-4o-mini', value: 'gpt-4o-mini' },
  { label: 'gpt-3.5-turbo', value: 'gpt-3.5-turbo' },
  { label: 'deepseek-chat', value: 'deepseek-chat' },
  { label: 'deepseek-coder', value: 'deepseek-coder' },
  { label: 'moonshot-v1-8k', value: 'moonshot-v1-8k' },
  { label: 'llama3.2', value: 'llama3.2' },
  { label: 'qwen2.5', value: 'qwen2.5' },
  { label: 'qwen3-coder-plus', value: 'qwen3-coder-plus' }
]

// Common API URL presets
const apiUrlOptions = [
  { label: '阿里云 DashScope', value: 'https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions' },
  { label: 'OpenAI', value: 'https://api.openai.com/v1/chat/completions' },
  { label: 'DeepSeek', value: 'https://api.deepseek.com/v1/chat/completions' },
  { label: 'Moonshot (Kimi)', value: 'https://api.moonshot.cn/v1/chat/completions' },
  { label: 'Ollama (本地)', value: 'http://localhost:11434/v1/chat/completions' }
]

watch(() => props.show, async (newVal) => {
  if (newVal) {
    try {
      const config = await invoke<AIConfig>('get_ai_config')
      formModel.value = config
    } catch (e) {
      console.error('Failed to load AI config:', e)
    }
  }
})

async function handleSave() {
  loading.value = true
  try {
    await invoke('save_ai_config', { config: formModel.value })
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
    :title="t('ai.config_title')"
    style="width: 550px"
  >
    <NForm :model="formModel" label-placement="left" label-width="100">
      <NFormItem :label="t('ai.api_url')" path="api_url">
        <NSelect 
          v-model:value="formModel.api_url" 
          :options="apiUrlOptions"
          filterable
          tag
          :placeholder="t('ai.api_url_placeholder')"
        />
      </NFormItem>
      
      <NFormItem :label="t('ai.api_key')" path="api_key">
        <NInput 
          v-model:value="formModel.api_key" 
          type="password"
          show-password-on="click"
          :placeholder="t('ai.api_key_placeholder')" 
        />
      </NFormItem>
      
      <NFormItem :label="t('ai.model')" path="model">
        <NSelect 
          v-model:value="formModel.model" 
          :options="modelOptions"
          filterable
          tag
          :placeholder="t('ai.model_placeholder')"
        />
      </NFormItem>
      
      <div class="tip">
        {{ t('ai.config_tip') }}
      </div>
    </NForm>

    <template #footer>
      <NSpace justify="end">
        <NButton @click="emit('update:show', false)">{{ t('common.cancel') }}</NButton>
        <NButton type="primary" @click="handleSave" :loading="loading">
          {{ t('common.save') }}
        </NButton>
      </NSpace>
    </template>
  </NModal>
</template>

<style scoped>
.tip {
  font-size: 12px;
  color: var(--n-text-color-3);
  margin-top: 8px;
}
</style>
