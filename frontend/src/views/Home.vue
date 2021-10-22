<template>
    <el-container>
        <el-aside width="80px">
            <el-menu :default-active="defaultActiveIndex" router :collapse="true" @select="handleSelect">
                <template v-for="(item,index) in $router.options.routes">
                    <el-menu-item v-if="item.leaf && item.children && item.children.length"
                                  :index="item.children[0].path"
                                  :class="$route.path===item.children[0].path?'is-active':''" :key="index+'1'">
                        <i :class="item.iconCls"></i>
                        <template #title>{{ $t(item.children[0].name) }}</template>
                    </el-menu-item>
                </template>
            </el-menu>
        </el-aside>

        <el-main>
            <router-view></router-view>
        </el-main>

    </el-container>
</template>

<script>

export default {
    data() {
        return {
            defaultActiveIndex: "0",
        };
    },
    methods: {
        handleSelect(index) {
            this.defaultActiveIndex = index;
        },
    }
}
</script>


<style scoped>

.container {
    position: absolute;
    top: 0px;
    bottom: 0px;
    width: 100%;
}

.main {
    display: flex;
}

</style>
