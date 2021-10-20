import {createApp} from 'vue'
import App from './App.vue'
import ElementUI from 'element-plus'
import 'element-plus/dist/index.css'
import router from './router'
import {createI18n} from 'vue-i18n'
import Video from 'video.js'
import video_zhCN from 'video.js/dist/lang/zh-CN.json'
import video_en from 'video.js/dist/lang/en.json'
import zhCN from './lang/zh'
import en from './lang/en'
import 'video.js/dist/video-js.css'

Video.addLanguage('zh-CN', video_zhCN);
Video.addLanguage('zh-CN', zhCN.video_js_playlist_ui);
Video.addLanguage('en', video_en);
Video.addLanguage('en', en.video_js_playlist_ui);

const i18n = createI18n({
    "locale": navigator.language.toLowerCase(),
    "messages": {
        'zh-cn': zhCN,
        'en-us': en
    }
})

const app = createApp(App)

app.config.globalProperties.$video = Video

app
    .use(router)
    .use(ElementUI)
    .use(i18n)
    .mount('#app')
