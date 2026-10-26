<script setup>
const props = defineProps({
    parentPage: {
        type: String,
        default: "",
    },
});
import { useRoute, useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
// import { ElMessageBox } from 'element-plus'
import { isOnGithub } from "../assets/tools.js";
const { t, locale } = useI18n();
const route = useRoute();
const router = useRouter();
const robotId = route.params.robotId;
function gotoDemo(id, name) {
    if (isOnGithub()) {
        ElMessageBox.alert(
            t("home.demoUnvailableContent"),
            t("home.demoUnvailableTitle"),
            {
                dangerouslyUseHTMLString: true,
                // if you want to disable its autofocus
                // autofocus: false,
                confirmButtonText: "OK",
                callback: (action) => {
                    // ElMessage({
                    //     type: 'info',
                    //     message: `action: ${action}`,
                    // })
                },
            },
        );
    } else {
        router.push({
            name: "subflow",
            params: { id: id, name: name, robotId: robotId },
        });
    }
}
// console.log("parentPage=" + props.parentPage);
</script>
<template>
    <!-- Stop propagation so clicks on the demo links don't also trigger a
         click handler on a clickable ancestor card (e.g. RobotDetail's
         dialog-flows card, which pushes to `mainflows` and would override
         the `subflow` navigation pushed by gotoDemo). -->
    <div class="demo-links" @click.stop>
        <strong>{{ $t("home.demo") }}</strong
        >:
        <template v-if="props.parentPage == 'home'">
            <ol>
                <li>
                    <el-link
                        type="success"
                        @click="gotoDemo('demo-repay', 'UmVwYXkgRGVtbw==')"
                        >{{ $t("home.demo1") }}</el-link
                    >
                </li>
                <li>
                    <el-link
                        type="success"
                        @click="
                            gotoDemo(
                                'demo-collect',
                                'SW5mb3JtYXRpb24gQ29sbGVjdGlvbiBEZW1v',
                            )
                        "
                        >{{ $t("home.demo2") }}</el-link
                    >
                </li>
                <li>
                    <el-link
                        type="success"
                        @click="
                            gotoDemo(
                                'demo-notify',
                                'T25lIFNlbnRlbmNlIE5vdGlmaWNhdGlvbiBEZW1v',
                            )
                        "
                        >{{ $t("home.demo3") }}</el-link
                    >
                </li>
            </ol>
        </template>
        <template v-else>
            <el-link
                type="success"
                @click="gotoDemo('demo-repay', 'UmVwYXkgRGVtbw==')"
                >{{ $t("home.demo1") }}</el-link
            >
            <el-link
                type="success"
                @click="
                    gotoDemo(
                        'demo-collect',
                        'SW5mb3JtYXRpb24gQ29sbGVjdGlvbiBEZW1v',
                    )
                "
                >{{ $t("home.demo2") }}</el-link
            >
            <el-link
                type="success"
                @click="
                    gotoDemo(
                        'demo-notify',
                        'T25lIFNlbnRlbmNlIE5vdGlmaWNhdGlvbiBEZW1v',
                    )
                "
                >{{ $t("home.demo3") }}</el-link
            >
        </template>
    </div>
</template>
<style scoped>
.demo-links .el-link {
    margin-right: 12px;
}
</style>
