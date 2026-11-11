import {computed, reactive, ref, watch, type Ref} from 'vue'
import {explore, type ExplorerItem} from './api'
import {preferences} from './preferences'

const directories = reactive(new Map<string, {items: ExplorerItem[]; includeOther: boolean}>())
const requests = new Map<string, {promise: Promise<ExplorerItem[]>; includeOther: boolean}>()
const collator = new Intl.Collator(undefined, {numeric: true, sensitivity: 'base'})
const audioExtensions = new Set(['aac', 'flac', 'm4a', 'mp3', 'opus', 'wav'])
export const canWrite = ref(false)

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

export function libraryRoute(path: string, other = false) {
    return {name: 'library', query: path === '/' ? {} : {dir: path, other: other ? '1' : undefined}}
}

export function playerRoute(dir: string, file: string) {
    return {name: 'player', query: {dir, file}}
}

export function extension(name: string): string {
    const dot = name.lastIndexOf('.')
    return dot > name.lastIndexOf('/') + 1 ? name.slice(dot + 1).toLowerCase() : ''
}

export function isAudio(name: string): boolean {
    return audioExtensions.has(extension(name))
}

export function title(name: string): string {
    const dot = name.lastIndexOf('.')
    return dot > 0 ? name.slice(0, dot) : name
}

function fileUrl(dir: string, name: string): string {
    return '/file' + joinPath(dir, name).split('/').map(encodeURIComponent).join('/')
}

export function mediaUrl(dir: string, item: ExplorerItem): string {
    const version = new URLSearchParams()
    if (item.modified !== undefined) version.set('v', String(item.modified))
    if (item.size !== undefined) version.set('size', String(item.size))
    const query = version.toString()
    return fileUrl(dir, item.name) + (query ? '?' + query : '')
}

export async function findUnlistedFile(dir: string, name: string, signal: AbortSignal): Promise<ExplorerItem | undefined> {
    if (dir === '/' || !name || name === '.' || name === '..' || /[/\\\0]/.test(name)) return
    const item: ExplorerItem = {name, type: 'other'}
    if (!visible(item)) return

    const response = await fetch(fileUrl(dir, name), {method: 'HEAD', cache: 'no-store', signal})
    if (response.status === 404) return
    if (!response.ok) throw new Error(String(response.status))
    const size = response.headers.get('content-length')
    const modified = Date.parse(response.headers.get('last-modified') ?? '')
    if (size !== null) item.size = Number(size)
    if (Number.isFinite(modified)) item.modified = modified
    return item
}

export function conversionUrl(source: string, endpoint: 'transcode' | 'thumbnail'): string {
    return source.replace(/^\/file\//, '/api/' + endpoint + '/')
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

export function orderedFiles(items: ExplorerItem[], kind: 'file' | 'other' = 'file'): ExplorerItem[] {
    return items.filter((item) => item.type === kind && visible(item)).sort((a, b) => {
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
    return (directories.get(path)?.items ?? []).filter((item) => item.type === 'dir' && (path === '/' || visible(item)))
        .sort(compareNames)
}

export function loadDirectory(path: string, {refresh = false, includeOther}: {refresh?: boolean; includeOther?: boolean} = {}): Promise<ExplorerItem[]> {
    const pending = requests.get(path)
    const cached = directories.get(path)
    // 刷新上传或新建后的目录时沿用该目录当前的显示范围。
    includeOther ??= refresh && (pending?.includeOther ?? cached?.includeOther ?? false)
    if (pending && !refresh && (!includeOther || pending.includeOther)) return pending.promise
    if (cached && !refresh && (!includeOther || cached.includeOther)) return Promise.resolve(cached.items)
    const promise = explore(path, includeOther).then(({items, canWrite: writable}) => {
        if (requests.get(path)?.promise === promise) {
            canWrite.value = writable
            directories.set(path, {items, includeOther})
        }
        return items
    }).finally(() => {
        if (requests.get(path)?.promise === promise) requests.delete(path)
    })
    requests.set(path, {promise, includeOther})
    return promise
}

export function useDirectory(path: Readonly<Ref<string>>, includeOther?: Readonly<Ref<boolean>>) {
    const items = computed(() => directories.get(path.value)?.items ?? [])
    const pending = ref(false)
    const error = ref('')
    let attempt = 0

    async function load(refresh: boolean) {
        const id = ++attempt
        pending.value = true
        error.value = ''
        try {
            await loadDirectory(path.value, {refresh, includeOther: includeOther?.value ?? false})
        } catch (cause) {
            if (id === attempt) error.value = String((cause as Error).message || cause)
        } finally {
            if (id === attempt) pending.value = false
        }
    }

    watch([path, () => includeOther?.value], ([directory, expanded], [previousDirectory]) => {
        void load(directory !== previousDirectory || !!expanded)
    }, {immediate: true})
    return {items, pending, error, reload: () => load(true)}
}
