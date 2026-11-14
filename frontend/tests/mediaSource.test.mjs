import assert from 'node:assert/strict'
import {test} from 'node:test'
import {createServer} from 'vite'
import {effectScope, ref, watch} from 'vue'

test('retry replaces a stale playback URL with the current media revision', {timeout: 5000}, async (context) => {
    const file = '/file/share/clip.wav?v=7'
    let revision = '123'
    let headRequests = 0
    context.mock.method(globalThis, 'fetch', async (url, options) => {
        if (options.method === 'HEAD') {
            assert.equal(url, file)
            assert.equal(options.cache, 'no-store')
            headRequests++
            return new Response(null, {headers: {'X-Media-Revision': revision}})
        }
        assert.equal(options.headers.Range, 'bytes=0-3')
        const requested = new URL(url, 'http://localhost').searchParams.get('revision')
        return new Response(null, {status: requested === revision ? 206 : 409})
    })
    const location = Object.getOwnPropertyDescriptor(globalThis, 'location')
    Object.defineProperty(globalThis, 'location', {configurable: true, value: new URL('http://localhost')})
    const server = await createServer({server: {middlewareMode: true, ws: false}})
    const scope = effectScope()
    try {
        const {useMediaSource} = await server.ssrLoadModule('/src/mediaSource.ts')
        const media = scope.run(() => useMediaSource(ref(file), () => {}))
        const loaded = () => new Promise(resolve => {
            const stop = watch(media.loading, loading => {
                if (!loading) {
                    stop()
                    resolve()
                }
            })
        })
        const requestRange = async () => (await fetch(media.src.value, {headers: {Range: 'bytes=0-3'}})).status
        await loaded()
        assert.equal(media.src.value, file + '&revision=123')
        assert.equal(await requestRange(), 206)

        revision = '456'
        assert.equal(await requestRange(), 409)
        media.reload()
        assert.equal(media.src.value, '')
        await loaded()
        assert.equal(media.src.value, file + '&revision=456')
        assert.equal(await requestRange(), 206)
        assert.equal(headRequests, 2)
    } finally {
        scope.stop()
        await server.close()
        if (location) Object.defineProperty(globalThis, 'location', location)
        else delete globalThis.location
    }
})

test('switching files and retrying unload a native HLS stream before HEAD', {timeout: 5000}, async (context) => {
    const server = await createServer({server: {middlewareMode: true, ws: false}})
    const scope = effectScope()
    try {
        const {unloadMedia, useMediaSource} = await server.ssrLoadModule('/src/mediaSource.ts')
        const oldStream = '/api/transcode/share/old.mp4?revision=123'
        const element = {
            src: oldStream,
            paused: false,
            loadCalls: 0,
            pause() { this.paused = true },
            removeAttribute(name) {
                assert.equal(name, 'src')
                delete this.src
            },
            load() { this.loadCalls++ },
        }
        const headCalls = []
        context.mock.method(globalThis, 'fetch', async (_, options) => {
            headCalls.push([options.method, element.paused, element.src, element.loadCalls])
            return new Response(null, {status: 404})
        })

        const source = ref('')
        const media = scope.run(() => useMediaSource(source, () => {
            if (element.src) unloadMedia(element)
        }))
        source.value = '/file/share/next.mp4'
        assert.deepEqual(headCalls, [['HEAD', true, undefined, 1]])

        element.src = '/api/transcode/share/next.mp4?revision=456'
        element.paused = false
        media.reload()
        assert.deepEqual(headCalls, [['HEAD', true, undefined, 1], ['HEAD', true, undefined, 2]])
        await Promise.resolve()
        assert.equal(media.failed.value, true)
        assert.equal(element.src, undefined)
    } finally {
        scope.stop()
        await server.close()
    }
})
