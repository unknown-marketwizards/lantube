import assert from 'node:assert/strict'
import {test} from 'node:test'
import {createServer} from 'vite'
import {effectScope, ref, watch} from 'vue'

test('retry updates chapters in place and reloads media only when the revision is stale', {timeout: 5000}, async (context) => {
    const responses = []
    context.mock.method(globalThis, 'fetch', async () => {
        const response = responses.shift()
        assert.ok(response)
        return response
    })
    const storage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')
    Object.defineProperty(globalThis, 'localStorage', {
        configurable: true,
        value: {getItem: () => null},
    })
    const server = await createServer({server: {middlewareMode: true, ws: false}})
    const scope = effectScope()
    try {
        let mediaReloads = 0
        const chapterSet = title => Response.json({version: title, chapters: [{startMs: 0, title}]})
        const conflict = () => new Response(null, {status: 409})
        const {useChapters} = await server.ssrLoadModule('/src/chapters.ts')
        const revision = ref('123')
        responses.push(chapterSet('initial'))
        const chapters = scope.run(() => useChapters(ref('/share/video.mp4'), revision, () => { mediaReloads++ }))
        const loaded = () => new Promise(resolve => {
            const stop = watch(chapters.loading, loading => {
                if (!loading) {
                    stop()
                    resolve()
                }
            })
        })
        await loaded()

        responses.push(new Response(null, {status: 400}))
        assert.equal(await chapters.save([{startMs: 0, title: 'initial'}, {startMs: 3000, title: 'out of range'}]), false)
        assert.equal(chapters.issue.value, 'chapterTimeInvalid')
        assert.equal(chapters.chapters.value.length, 1)

        responses.push(conflict(), chapterSet('another editor'))
        assert.equal(await chapters.save([{startMs: 0, title: 'my edit'}]), false)
        assert.equal(chapters.issue.value, 'chapterConflict')
        chapters.reload()
        await loaded()
        assert.equal(chapters.chapters.value[0].title, 'another editor')
        assert.equal(chapters.issue.value, undefined)
        assert.equal(mediaReloads, 0)

        responses.push(conflict())
        chapters.reload()
        await loaded()
        assert.equal(mediaReloads, 1)
        assert.equal(responses.length, 0)
    } finally {
        scope.stop()
        await server.close()
        if (storage) Object.defineProperty(globalThis, 'localStorage', storage)
        else delete globalThis.localStorage
    }
})
