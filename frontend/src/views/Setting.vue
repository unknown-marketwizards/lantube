<template>
    <el-row>
        <el-select v-model="locale" :placeholder="$t('setting.select_please')" @change="onChange">
            <el-option
                v-for="item in options"
                :key="item.value"
                :label="item.label"
                :value="item.value">
            </el-option>
        </el-select>
    </el-row>
</template>

<script lang="ts">
import {defineComponent} from 'vue'

export default defineComponent({
    methods: {
        onChange() {
            this.$i18n.locale = this.locale
        }
    },
    data() {
        return {
            options: [{
                value: 'en-us',
                label: 'English'
            }, {
                value: 'zh-cn',
                label: '简体中文'
            }],
            locale: this.$i18n.locale
        }
    }
})
</script>

<style scoped>

</style>