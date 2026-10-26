import { createI18n } from "vue-i18n";
// import messages from './lang.js'
import zh from './zh.js'
import en from './en.js'
// const language = ((navigator.language ? navigator.language : navigator.userLanguage) || 'en').toLowerCase()
function detectLocale() {
    const queryLang = new URLSearchParams(window.location.search).get('lang')
    if (queryLang && ['en', 'zh'].includes(queryLang)) return queryLang

    const savedLang = localStorage.getItem('lang')
    if (savedLang && ['en', 'zh'].includes(savedLang)) return savedLang

    const browserLang = ((navigator.language ? navigator.language : navigator.userLanguage) || 'en').toLowerCase()
    if (browserLang.includes('zh')) return 'zh'
    if (browserLang.includes('en')) return 'en'

    return 'en' // fallback
}
const i18n = createI18n({
    fallbackLocale: 'en',
    globalInjection: true,
    legacy: false,
    locale: detectLocale(),
    // locale: language.split('-')[0] || 'en',
    // messages,
    messages: {
        zh,
        en
    }
})

export default i18n;