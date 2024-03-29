<template>
    <div class="player-container">
        <video ref="videoPlayer" class="video-js vjs-big-play-centered"></video>
        <div class="vjs-playlist playlist-container">
        </div>
    </div>
    <el-dialog v-model="settingVisible">
        <el-radio-group v-model="playMode" size="small" @change="modeChange">
            <el-radio-button label="auto">{{ $t('videos.play_mode_auto') }}</el-radio-button>
            <el-radio-button label="manual">{{ $t('videos.play_mode_manual') }}</el-radio-button>
            <el-radio-button label="20">{{ $t('videos.play_mode_20m') }}</el-radio-button>
            <el-radio-button label="30">{{ $t('videos.play_mode_30m') }}</el-radio-button>
            <el-radio-button label="40">{{ $t('videos.play_mode_40m') }}</el-radio-button>
            <el-radio-button label="60">{{ $t('videos.play_mode_60m') }}</el-radio-button>
        </el-radio-group>
    </el-dialog>
</template>

<script lang="ts">
import 'videojs-playlist'
import 'videojs-playlist-ui'
import 'videojs-playlist-ui/dist/videojs-playlist-ui.css'
import videojs from "video.js";
import {defineComponent} from "vue";

export default defineComponent({

    data: function () {
        return {
            player: null as videojs.Player | null,
            options: {
                language: this.$i18n.locale,
                autoplay: true,
                controls: true,
                playbackRates: [0.5, 1, 1.25, 1.5, 2, 3],

                sources: []
            },

            typeDict: {
                mp4: 'video/mp4',
                webm: 'video/webm',
                ogv: 'video/ogg'
            },

            settingVisible: false,
            playMode: 'auto',
            timeStart: null as Date | null
        }
    },
    methods: {
        checkPause() {
            if (!this.player)
                return
            if (!this.timeStart)
                return
            if (this.playMode === 'auto' || this.playMode === 'manual')
                return

            let end: Date = new Date()
            const diff = (end - this.timeStart) / (1000 * 60)
            let n = parseInt(this.playMode)
            if (diff > n) {
                this.player.pause()
            }
            setTimeout(() => {
                this.checkPause()
            }, 10000)
        },
        modeChange() {
            switch (this.playMode) {
                case "auto":
                    this.timeStart = null
                    break
                case "manual":
                    this.timeStart = null
                    break
                case "20":
                case "30":
                case "40":
                case "60":
                    this.timeStart = new Date()
                    break
            }
            this.checkPause()
        }
    },
    mounted() {
        let playlist = this.$route.query.playlist
        const index = parseInt(this.$route.query.index)

        if (typeof playlist === "string") {
            playlist = [playlist]
        }

        let getFileName = function (path: string) {
            const pos1 = path.lastIndexOf('/');
            const pos2 = path.lastIndexOf('\\');
            const pos = Math.max(pos1, pos2);
            if (pos < 0)
                return path
            else
                return path.substring(pos + 1)
        }

        let getExt = function (path: string) {
            const index = path.lastIndexOf(".")
            return path.substr(index + 1).toLowerCase()
        }

        let list = []
        for (let i in playlist) {
            let ext = getExt(playlist[i])
            list.push({
                name: getFileName(playlist[i]),
                sources: [{
                    src: playlist[i],
                    type: this.typeDict[ext]
                }],
            })
        }

        let that = this
        this.player = this.$video(this.$refs.videoPlayer, this.options, function onPlayerReady() {
            if (!that.player)
                return

            const customButton = that.$video.getComponent("Button")
            class pipButton extends customButton {
                constructor (player, options) {
                    super(player, options);
                    this.controlText(that.$t('videos.setting'));
                }
                handleClick() {
                    that.settingVisible = true
                }
                buildCSSClass() {
                    return "vjs-custom-control vjs-control vjs-button icon el-icon-setting";
                }
            }

            that.$video.registerComponent("settingButton", pipButton);
            that.player.getChild('controlBar').addChild('settingButton', {}, 1);

            that.player.playlist(list)
            that.player.playlistUi()

            that.player.playlist.autoadvance(0)
            that.player.playlist.currentItem(index)

            that.player.on("loadstart", function () {
                if (that.player && that.playMode === 'manual') {
                    that.player.pause()
                }
            })
        })
    },
    beforeUnmount() {
        if (this.player) {
            let el = this.player.playlistMenu.el(); // Catch cases where the menu may have been disposed elsewhere or the
            // element removed from the DOM.

            if (el) {
                this.player.playlistMenu.dispose();
                videojs.dom.emptyEl(el); // Put the element back in its place.
            }
            this.player.dispose()
        }
    }
})
</script>

<style scoped>

.player-container {
    background: #1a1a1a;
    color: #00FF00;
    overflow: auto;
}

.video-js {
    float: left;
    width: 70% !important;
    height: 720px;
}

.vjs-playlist {
    float: left;
    width: 30%;
    height: 720px;
}

.vjs-playlist .vjs-selected {
    border: 1px solid #00FF00;
}

.vjs-playlist .vjs-selected img {
    opacity: .5;
}
</style>