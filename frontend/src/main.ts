import {createApp, watchEffect} from 'vue'
import {createI18n} from 'vue-i18n'
import '@fontsource-variable/geist'
import './assets/theme.css'
import App from './App.vue'
import router from './router'
import {preferences} from './preferences'
import zh from './lang/zh'
import en from './lang/en'

const i18n = createI18n({
    legacy: false,
    locale: preferences.language,
    fallbackLocale: 'en',
    messages: {zh, en},
})

watchEffect(() => {
    i18n.global.locale.value = preferences.language
    document.documentElement.lang = preferences.language === 'zh' ? 'zh-CN' : 'en'
})

createApp(App).use(router).use(i18n).mount('#app')
