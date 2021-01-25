import Vue from 'vue'
import App from './App.vue'
import ElementUI from 'element-ui'
import 'element-ui/lib/theme-chalk/index.css'
import router from './router'
import VueI18n from 'vue-i18n'
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

Vue.prototype.$video = Video

Vue.use(VueI18n)

const i18n = new VueI18n({
    "locale": navigator.language.toLowerCase(),
    "messages": {
        'zh-cn': zhCN,
        'en-us': en
    }
})

Vue.use(ElementUI)

Vue.config.productionTip = false

new Vue({
    router,
    i18n,
    render: h => h(App),
}).$mount('#app')
