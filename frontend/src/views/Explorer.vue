<template>

    <el-drawer
        :title="$t('explorer.upload_title') + ' ' + currentPath"
        v-model="drawer"
        direction="rtl">
        <el-upload
            class="upload-demo"
            drag
            action="/api/upload"
            :data="{'path': currentPath}"
            :on-success="onUpload"
            multiple>
            <i class="el-icon-upload"></i>
            <div class="el-upload__text">{{
                    $t('explorer.upload_text')
                }}<em>{{ $t('explorer.upload_text_em') }}</em></div>
            <div class="el-upload__tip" slot="tip"></div>
        </el-upload>
    </el-drawer>

    <el-row style="text-align: left;">
        <el-col>
            <el-breadcrumb separator-class="el-icon-arrow-right" style="margin-left: 10px">
                <el-breadcrumb-item v-for="(item, i) in filePath" :key="item + i">
                    <el-button type="text" @click="onClickBreadcrumb(i)">{{
                            item === '/' ? $t('explorer.root_dir') : item
                        }}
                    </el-button>
                </el-breadcrumb-item>
                <el-breadcrumb>
                    <el-button @click="createDir" type="text" style="color: red"><i class="el-icon-folder-add"></i>
                    </el-button>
                    <el-button @click="drawer = true" type="text" style="color: red"><i class="el-icon-plus"></i>
                    </el-button>
                </el-breadcrumb>
            </el-breadcrumb>
        </el-col>
    </el-row>
    <el-row>
        <el-table :data="fileData" @row-click="onClick" v-loading="loading">
            <el-table-column sortable
                             prop="name"
                             :label="$t('explorer.name')">
                <template #default="scope">
                    <i :class="scope.row.type === 'dir' ? 'el-icon-folder' : 'el-icon-document'"></i>
                    <span style="margin-left: 10px">{{ scope.row.name }}</span>
                </template>
            </el-table-column>
        </el-table>
    </el-row>
</template>

<script>
import API from '../api'
import path_browserify from 'path-browserify'

export default {
    data() {
        return {
            fileData: [],
            loading: false,

            filePath: ['/'],

            drawer: false
        }
    },
    computed: {
        currentPath() {
            let p = ''
            for (let i in this.filePath) {
                p = path_browserify.join(p, this.filePath[i])
            }
            return p
        },
    },
    methods: {
        onUpload() {
            this.reload('')
        },
        createDir() {
            this.$prompt(this.$t('explorer.dir_name_please'),
                this.$t('explorer.new_dir_on') + ' ' + this.currentPath, {
                    confirmButtonText: this.$t('confirm'),
                    cancelButtonText: this.$t('cancel'),
                }).then(({value}) => {
                let that = this
                let path = path_browserify.join(this.currentPath, value)
                API.mkdir(path).then(function () {
                    that.reload('')
                    that.$message({
                        type: 'success',
                        message: that.$t('explorer.new_dir_success')
                    });
                }, function (err) {
                    that.loading = false
                    that.$message.error({showClose: true, message: err.toString(), duration: 2000})
                }).catch(function (err) {
                    that.loading = false
                    that.$message.error({showClose: true, message: err.toString(), duration: 2000})
                })

            }).catch(() => {
            })
        },
        onClickBreadcrumb(i) {
            this.filePath.splice(i + 1, this.filePath.length - i);
            this.reload('')
        },
        onClick: function (row) {
            if (row.type === 'dir') {
                this.reload(row.name)
            } else {
                const playlist = []
                let playIndex = 0;
                let index = 0;
                for (let i in this.fileData) {
                    if (this.fileData[i].type === 'dir') {
                        continue
                    }
                    if (this.fileData[i].name === row.name) {
                        playIndex = index
                    }
                    let path = path_browserify.join('file', this.currentPath, this.fileData[i].name)
                    playlist.push(path)
                    index++
                }

                this.$router.push({path: '/videos', query: {playlist: playlist, index: playIndex}})
            }
        },
        reload(name) {
            let that = this
            this.loading = true
            let path = path_browserify.join(this.currentPath, name)

            API.explorer(path).then(function (result) {
                that.loading = false
                that.fileData = result ? result : []
                if (name !== '') {
                    that.filePath.push(name)
                }
            }, function (err) {
                that.loading = false
                that.$message.error({showClose: true, message: err.toString(), duration: 2000})
            }).catch(function (err) {
                that.loading = false
                that.$message.error({showClose: true, message: err.toString(), duration: 2000})
            })
        }
    },
    mounted() {
        this.reload('')
    }
}
</script>

<style scoped>

</style>