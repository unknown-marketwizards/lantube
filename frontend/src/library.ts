import {computed, reactive, ref, watch, type Ref} from 'vue'
import {explore, type ExplorerItem} from './api'
import {preferences} from './preferences'

const directories = reactive(new Map<string, ExplorerItem[]>())
const requests = new Map<string, Promise<ExplorerItem[]>>()
const collator = new Intl.Collator(undefined, {numeric: true, sensitivity: 'base'})
const audioExtensions = new Set(['aac', 'flac', 'm4a', 'mp3', 'ogg', 'opus', 'wav'])

function orderName(name: string): string {
    return name.replace(/第([一二两三四五六七八九]?十[一二两三四五六七八九]?|[一二两三四五六七八九])(?=[季集部卷章])/g, (_, numeral: string) => {
        const digit = (value: string) => value === '两' ? 2 : '零一二三四五六七八九'.indexOf(value)
        const [first, last] = numeral.split('十')
        const number = last === undefined ? digit(first) : (first ? digit(first) : 1) * 10 + digit(last)
        return '第' + number
    })
}

function compareNames(a: ExplorerItem, b: ExplorerItem) {
    return collator.compare(orderName(a.name), orderName(b.name)) || collator.compare(a.name, b.name)
}

function visible(item: ExplorerItem) {
    return preferences.showHidden || !item.name.startsWith('.')
}

export function directoryPath(value: unknown): string {
    return typeof value === 'string' && value.startsWith('/') ? value : '/'
}

export function joinPath(parent: string, name: string): string {
    return (parent === '/' ? '' : parent) + '/' + name
}

export function parentPath(path: string): string {
    return path.slice(0, path.lastIndexOf('/')) || '/'
}

export function pathName(path: string): string {
    return path.slice(path.lastIndexOf('/') + 1)
}

export function libraryRoute(path: string) {
    return {name: 'library', query: path === '/' ? {} : {dir: path}}
}

export function playerRoute(dir: string, file: string) {
    return {name: 'player', query: {dir, file}}
}

export function extension(name: string): string {
    return name.slice(name.lastIndexOf('.') + 1).toLowerCase()
}

export function isAudio(name: string): boolean {
    return audioExtensions.has(extension(name))
}

export function title(name: string): string {
    const dot = name.lastIndexOf('.')
    return dot > 0 ? name.slice(0, dot) : name
}

export function mediaUrl(dir: string, item: ExplorerItem): string {
    const path = joinPath(dir, item.name).split('/').map(encodeURIComponent).join('/')
    const revision = item.modified ?? item.size
    return '/file' + path + (revision === undefined ? '' : '?v=' + revision)
}

export function formatSize(size?: number): string {
    if (size === undefined) return ''
    if (size < 1024) return size + ' B'
    const units = ['KB', 'MB', 'GB', 'TB']
    let value = size / 1024
    let unit = 0
    while (value >= 1024 && unit < units.length - 1) {
        value /= 1024
        unit++
    }
    return new Intl.NumberFormat(undefined, {maximumFractionDigits: value < 10 ? 1 : 0})
        .format(value) + ' ' + units[unit]
}

export function orderedFiles(items: ExplorerItem[]): ExplorerItem[] {
    return items.filter((item) => item.type === 'file' && visible(item)).sort((a, b) => {
        if (preferences.sort === 'recent') {
            return (b.modified ?? 0) - (a.modified ?? 0) || compareNames(a, b)
        }
        if (preferences.sort === 'size') {
            return (b.size ?? 0) - (a.size ?? 0) || compareNames(a, b)
        }
        return compareNames(a, b)
    })
}

export function folders(path: string): ExplorerItem[] {
    return (directories.get(path) ?? []).filter((item) => item.type === 'dir' && (path === '/' || visible(item)))
        .sort(compareNames)
}

export function loadDirectory(path: string, refresh = false): Promise<ExplorerItem[]> {
    const pending = requests.get(path)
    if (pending && !refresh) return pending
    const cached = directories.get(path)
    if (cached && !refresh) return Promise.resolve(cached)
    const request = explore(path).then((items) => {
        if (requests.get(path) === request) directories.set(path, items)
        return items
    }).finally(() => {
        if (requests.get(path) === request) requests.delete(path)
    })
    requests.set(path, request)
    return request
}

export function useDirectory(path: Readonly<Ref<string>>) {
    const items = computed(() => directories.get(path.value) ?? [])
    const pending = ref(false)
    const error = ref('')
    let attempt = 0

    async function reload() {
        const id = ++attempt
        pending.value = true
        error.value = ''
        try {
            await loadDirectory(path.value, true)
        } catch (cause) {
            if (id === attempt) error.value = String((cause as Error).message || cause)
        } finally {
            if (id === attempt) pending.value = false
        }
    }

    watch(path, reload, {immediate: true})
    return {items, pending, error, reload}
}
