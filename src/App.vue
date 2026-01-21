<script setup lang="ts">
import { ref, h, computed, onMounted, watch } from 'vue'
import { RouterView, useRouter } from 'vue-router'
import { useI18n } from 'vue-i18n'
import {
  NConfigProvider,
  NLayout,
  NLayoutSider,
  NLayoutContent,
  NMenu,
  NIcon,
  NDropdown,
  NMessageProvider,
  NDialogProvider,
  darkTheme,
  type MenuOption
} from 'naive-ui'
import {
  ServerOutline,
  SunnyOutline,
  MoonOutline,
  DesktopOutline,
  LanguageOutline
} from '@vicons/ionicons5'

const router = useRouter()
const { t, locale } = useI18n()
const collapsed = ref(true)

// 主题管理: 'light' | 'dark' | 'system'
const themeMode = ref<'light' | 'dark' | 'system'>('system')
const systemDark = ref(false)

// 检测系统主题
function detectSystemTheme() {
  systemDark.value = window.matchMedia('(prefers-color-scheme: dark)').matches
}

// 监听系统主题变化
onMounted(() => {
  detectSystemTheme()
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', detectSystemTheme)
  
  // 从本地存储恢复主题设置
  const saved = localStorage.getItem('recch-theme')
  if (saved && ['light', 'dark', 'system'].includes(saved)) {
    themeMode.value = saved as 'light' | 'dark' | 'system'
  }
  
  // Restore language
  const savedLang = localStorage.getItem('recch-lang')
  if (savedLang && ['en', 'zh-CN'].includes(savedLang)) {
      locale.value = savedLang as 'en' | 'zh-CN'
  }
})

// 保存主题设置
watch(themeMode, (val) => {
  localStorage.setItem('recch-theme', val)
})

// Save language setting
watch(locale, (val) => {
    localStorage.setItem('recch-lang', val)
})

// 计算当前是否使用暗色主题
const isDark = computed(() => {
  if (themeMode.value === 'system') {
    return systemDark.value
  }
  return themeMode.value === 'dark'
})

const currentTheme = computed(() => isDark.value ? darkTheme : null)

function renderIcon(icon: any) {
  return () => h(NIcon, null, { default: () => h(icon) })
}

// Menu options must be computed to react to locale changes
const menuOptions = computed<MenuOption[]>(() => [
  {
    label: t('menu.connections'),
    key: 'connections',
    icon: renderIcon(ServerOutline)
  }
])

// 主题切换选项
const themeOptions = computed(() => [
  { label: t('menu.theme_light'), key: 'light', icon: renderIcon(SunnyOutline) },
  { label: t('menu.theme_dark'), key: 'dark', icon: renderIcon(MoonOutline) },
  { label: 'System', key: 'system', icon: renderIcon(DesktopOutline) }
])

// Language options
const langOptions = [
    { label: 'English', key: 'en' },
    { label: '中文', key: 'zh-CN' }
]

function handleMenuUpdate(key: string) {
  if (key === 'home') {
    router.push('/')
  } else if (key === 'connections') {
    router.push('/connections')
  }
}

function handleThemeSelect(key: string) {
  themeMode.value = key as 'light' | 'dark' | 'system'
}

function handleLangSelect(key: string) {
    locale.value = key as 'en' | 'zh-CN'
}

const themeIcon = computed(() => {
  if (themeMode.value === 'light') return SunnyOutline
  if (themeMode.value === 'dark') return MoonOutline
  return DesktopOutline
})
</script>

<template>
  <NConfigProvider :theme="currentTheme">
    <NMessageProvider>
      <NDialogProvider>
        <NLayout has-sider style="height: 100vh;">
          <NLayoutSider
            bordered
            collapse-mode="width"
            :collapsed-width="64"
            :width="220"
            :collapsed="collapsed"
            show-trigger
            @collapse="collapsed = true"
            @expand="collapsed = false"
          >
            <div class="logo" :class="{ collapsed, dark: isDark }">
              <span v-if="!collapsed">RECCH</span>
              <span v-else>R</span>
            </div>
            <NMenu
              :collapsed="collapsed"
              :collapsed-width="64"
              :collapsed-icon-size="22"
              :options="menuOptions"
              default-value="home"
              @update:value="handleMenuUpdate"
            />
            
            <div class="bottom-actions">
                 <!-- Language Switcher -->
                <NDropdown
                    :options="langOptions"
                    @select="handleLangSelect"
                    trigger="click"
                    placement="top"
                >
                    <div class="action-btn" :class="{ collapsed }">
                        <NIcon size="20"><LanguageOutline /></NIcon>
                        <span v-if="!collapsed" style="margin-left: 8px;">
                            {{ locale === 'zh-CN' ? '中文' : 'English' }}
                        </span>
                    </div>
                </NDropdown>

                <!-- Theme Switcher -->
                <NDropdown
                    :options="themeOptions"
                    @select="handleThemeSelect"
                    trigger="click"
                    placement="top"
                >
                    <div class="action-btn" :class="{ collapsed }">
                    <NIcon size="20">
                        <component :is="themeIcon" />
                    </NIcon>
                    <span v-if="!collapsed" style="margin-left: 8px;">
                        {{ themeMode === 'light' ? t('menu.theme_light') : themeMode === 'dark' ? t('menu.theme_dark') : 'System' }}
                    </span>
                    </div>
                </NDropdown>
            </div>
          </NLayoutSider>
          <NLayoutContent content-style="padding: 24px;">
            <RouterView />
          </NLayoutContent>
        </NLayout>
      </NDialogProvider>
    </NMessageProvider>
  </NConfigProvider>
</template>

<style scoped>
.logo {
  height: 64px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 24px;
  font-weight: bold;
  color: #18a058;
  border-bottom: 1px solid rgba(0, 0, 0, 0.09);
  transition: all 0.3s;
}

.logo.dark {
  color: #63e2b7;
  border-bottom-color: rgba(255, 255, 255, 0.09);
}

.logo.collapsed {
  font-size: 28px;
}

.bottom-actions {
  position: absolute;
  bottom: 16px;
  left: 0;
  right: 0;
  padding: 0 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.action-btn {
  display: flex;
  align-items: center;
  justify-content: flex-start; /* Changed to start for better alignment */
  padding: 8px 12px;
  border-radius: 6px;
  cursor: pointer;
  transition: background-color 0.3s;
}

.action-btn:hover {
  background-color: rgba(0, 0, 0, 0.06);
}

.action-btn.collapsed {
  justify-content: center;
  padding: 8px;
}
</style>
