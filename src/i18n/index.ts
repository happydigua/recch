import { createI18n } from 'vue-i18n'
import en from './locales/en.json'
import zhCN from './locales/zh-CN.json'

// Type-define 'en' as the master schema for the resource
type MessageSchema = typeof en

// Detect system/browser language
function getSystemLocale(): 'en' | 'zh-CN' {
    const browserLang = navigator.language || (navigator as any).userLanguage || 'en'
    // Check if Chinese (zh, zh-CN, zh-TW, zh-Hans, zh-Hant, etc.)
    if (browserLang.toLowerCase().startsWith('zh')) {
        return 'zh-CN'
    }
    return 'en'
}

const i18n = createI18n<[MessageSchema], 'en' | 'zh-CN'>({
    legacy: false, // Composition API
    locale: getSystemLocale(), // Auto-detect system language
    fallbackLocale: 'en',
    messages: {
        en,
        'zh-CN': zhCN
    }
})

export default i18n

