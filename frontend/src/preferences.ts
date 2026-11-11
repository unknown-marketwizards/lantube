import {reactive, watch} from 'vue'

const storedLanguage = localStorage.getItem('lantube.language')
const storedView = localStorage.getItem('lantube.view')
const storedSort = localStorage.getItem('lantube.sort')

export const preferences = reactive({
    language: (storedLanguage === 'en' || storedLanguage === 'zh'
        ? storedLanguage
        : navigator.language.startsWith('zh') ? 'zh' : 'en') as 'en' | 'zh',
    view: (storedView === 'list' ? 'list' : 'grid') as 'grid' | 'list',
    sort: (storedSort === 'recent' || storedSort === 'size' ? storedSort : 'name') as 'name' | 'recent' | 'size',
    showHidden: localStorage.getItem('lantube.showHidden') === 'true',
})

watch(preferences, ({language, view, sort, showHidden}) => {
    localStorage.setItem('lantube.language', language)
    localStorage.setItem('lantube.view', view)
    localStorage.setItem('lantube.sort', sort)
    localStorage.setItem('lantube.showHidden', String(showHidden))
})
