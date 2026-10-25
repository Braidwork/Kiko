<script setup>
import { ref, reactive, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import {
    copyProperties,
    httpReq,
    persistRobotDetail,
} from "../../assets/tools.js";
import Demos from "../Demos.vue";
import EpArrowRightBold from "~icons/ep/arrow-right-bold";
import BiChatSquareDots from "~icons/bi/chat-square-dots";
import MaterialSymbolsBook5Outline from "~icons/material-symbols/book-5-outline";
import RiBardLine from "~icons/ri/bard-line";
import SolarDownloadOutline from "~icons/solar/download-outline";
import SolarRouting2Linear from "~icons/solar/routing-2-linear";
import EpSetting from "~icons/ep/setting";
import SolarDocumentTextLinear from "~icons/solar/document-text-linear";
import BiBoxArrowUpRight from "~icons/bi/box-arrow-up-right";
import EpDelete from "~icons/ep/delete";
import EpEditPen from "~icons/ep/edit-pen";
const { t, locale } = useI18n();
const route = useRoute();
const router = useRouter();
const robotId = route.params.robotId;
const fromPage = "robotDetail";
let robotNameForRestore = "";
const robotData = reactive({
    robotId: "",
    robotName: "",
    robotType: "",
    // 乐观锁版本（B-17）：详情接口返回，保存时原样带回。
    version: null,
});
const dialogFormVisible = ref(false);
const formLabelWidth = "90px";
const goBack = () => {
    router.push("/");
};
onMounted(async () => {
    const t = await httpReq(
        "GET",
        "robot/detail",
        { robotId: robotId },
        null,
        null,
    );
    if (t.status == 200 && t.data != null) {
        copyProperties(t.data, robotData);
        robotNameForRestore = robotData.robotName;
        persistRobotDetail(t.data);
    } else {
        ElMessage.error("Can NOT find robot information by robotId.");
    }
});
async function updateRobot() {
    const t = await httpReq("POST", "robot", null, null, robotData);
    if (t.status == 200) ElMessage.success("Changed successfully.");
    else ElMessage.error(t.err.message);
}
async function deleteRobot() {
    ElMessageBox.confirm(t("guide.delRoConfirm"), "Warning", {
        confirmButtonText: "OK",
        cancelButtonText: "Cancel",
        type: "warning",
    })
        .then(async () => {
            const t = await httpReq(
                "DELETE",
                "robot",
                { robotId: robotId },
                null,
                null,
            );
            if (t.status == 200) goBack();
            else ElMessage.error(t.err.message);
        })
        .catch(() => {});
}
const isZhLang = locale.value == "zh";
const getBotType = (type) => {
    if (type == "OutboundCallBot")
        return isZhLang ? "语音外呼机器人" : "Outbound call bot";
    else if (type == "InboundCallBot")
        return isZhLang ? "语音呼入机器人" : "Incoming call bot";
    else if (type == "TextBot")
        return isZhLang ? "文本机器人" : "Text chat bot";
    else return "";
};
</script>
<style scoped>
.robot-hero {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 16px;
    margin-bottom: 24px;
    padding: 28px 32px;
    border-radius: 18px;
    background:
        radial-gradient(circle at 90% 10%, rgba(139, 92, 246, 0.35), transparent 55%),
        linear-gradient(135deg, #4f46e5, #7c3aed);
    color: #fff;
    box-shadow: 0 12px 28px rgba(79, 70, 229, 0.22);
}

.robot-hero-info {
    min-width: 0;
}

.robot-name-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
}

.robot-name {
    font-size: 26px;
    font-weight: 700;
    line-height: 1.2;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.robot-type-badge {
    padding: 4px 14px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 600;
    color: #fff;
    background: rgba(255, 255, 255, 0.18);
    border: 1px solid rgba(255, 255, 255, 0.3);
}

.robot-id {
    margin-top: 8px;
    font-size: 13px;
    opacity: 0.75;
    word-break: break-all;
}

.robot-hero-actions {
    display: flex;
    align-items: center;
    gap: 10px;
}

.hero-btn {
    border: none;
    color: #4f46e5;
    font-weight: 600;
    border-radius: 10px;
}

.hero-btn.ghost {
    background: rgba(255, 255, 255, 0.14);
    color: #fff;
}

.hero-btn.ghost:hover {
    background: rgba(255, 255, 255, 0.25);
}

.robot-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 18px;
}

.robot-section-card {
    display: flex;
    align-items: flex-start;
    gap: 14px;
    background: #fff;
    border: 1px solid #eef1f6;
    border-radius: 16px;
    padding: 22px;
    color: inherit;
    text-decoration: none;
    cursor: pointer;
    transition:
        transform 0.25s ease,
        box-shadow 0.25s ease;
}

.robot-section-card:hover {
    transform: translateY(-3px);
    box-shadow: 0 12px 28px rgba(31, 45, 61, 0.1);
}

.section-icon {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 46px;
    height: 46px;
    border-radius: 12px;
    font-size: 22px;
}

.section-icon.indigo {
    color: #6366f1;
    background: #eef2ff;
}

.section-icon.sky {
    color: #0284c7;
    background: #e0f2fe;
}

.section-icon.amber {
    color: #d97706;
    background: #fef3c7;
}

.section-icon.rose {
    color: #e11d48;
    background: #ffe4e6;
}

.section-icon.emerald {
    color: #059669;
    background: #d1fae5;
}

.section-body {
    flex: 1;
    min-width: 0;
}

.section-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 16px;
    font-weight: 600;
    color: #1f2d3d;
}

.section-head .el-icon {
    font-size: 13px;
    color: #86909c;
    transition: transform 0.2s ease, color 0.2s ease;
}

.robot-section-card:hover .section-head .el-icon {
    transform: translateX(3px);
    color: #6366f1;
}

.section-desc {
    margin-top: 6px;
    font-size: 13px;
    line-height: 1.7;
    color: #86909c;
}

.section-links {
    margin-top: 10px;
    display: flex;
    flex-wrap: wrap;
    gap: 8px 18px;
    font-size: 13px;
}

.section-links a {
    color: #6366f1;
    text-decoration: none;
    display: inline-flex;
    align-items: center;
    gap: 4px;
}

.section-links a:hover {
    text-decoration: underline;
}

@media (max-width: 768px) {
    .robot-hero {
        padding: 22px;
    }

    .robot-name {
        font-size: 20px;
    }
}
</style>
<template>
    <div class="robot-hero">
        <div class="robot-hero-info">
            <div class="robot-name-row">
                <span class="robot-name" :title="robotData.robotName">{{
                    robotData.robotName
                }}</span>
                <span class="robot-type-badge">{{
                    getBotType(robotData.robotType)
                }}</span>
            </div>
            <div class="robot-id">
                {{ t("guide.robotId") }}: {{ robotId }}
            </div>
        </div>
        <div class="robot-hero-actions">
            <el-button
                class="hero-btn"
                @click="dialogFormVisible = true"
            >
                <el-icon style="margin-right: 6px"><EpEditPen /></el-icon>
                {{ t("guide.chRoNaBtn") }}
            </el-button>
            <el-button class="hero-btn ghost" @click="deleteRobot">
                <el-icon style="margin-right: 6px"><EpDelete /></el-icon>
                {{ t("guide.delRoNaBtn") }}
            </el-button>
        </div>
    </div>

    <div class="robot-grid">
        <!-- Dialog flows -->
        <div
            class="robot-section-card"
            @click="
                router.push({
                    name: 'mainflows',
                    params: { robotId: robotId },
                })
            "
        >
            <span class="section-icon indigo"><BiChatSquareDots /></span>
            <span class="section-body">
                <span class="section-head">
                    {{ $t("guide.title1") }}
                    <el-icon><EpArrowRightBold /></el-icon>
                </span>
                <span class="section-desc">
                    <Demos :parentPage="fromPage" />
                </span>
            </span>
        </div>

        <!-- Knowledge base -->
        <div class="robot-section-card">
            <span class="section-icon emerald">
                <MaterialSymbolsBook5Outline />
            </span>
            <span class="section-body">
                <span class="section-head">{{ t("menu.kb") }}</span>
                <span class="section-desc">{{ $t("guide.kbDesc") }}</span>
                <span class="section-links">
                    <router-link
                        :to="{ name: 'kbQA', params: { robotId: robotId } }"
                        >{{ t("menu.qa") }}<el-icon><EpArrowRightBold /></el-icon
                    ></router-link>
                    <router-link
                        :to="{ name: 'kbDoc', params: { robotId: robotId } }"
                        >{{ t("menu.doc") }}<el-icon><EpArrowRightBold /></el-icon
                    ></router-link>
                </span>
            </span>
        </div>

        <!-- Intents -->
        <div
            class="robot-section-card"
            @click="
                router.push({ name: 'intents', params: { robotId: robotId } })
            "
        >
            <span class="section-icon amber"><RiBardLine /></span>
            <span class="section-body">
                <span class="section-head">
                    {{ $t("guide.title2") }}
                    <el-icon><EpArrowRightBold /></el-icon>
                </span>
                <span class="section-desc">
                    {{ $t("guide.desc2") }}<br />{{ $t("guide.intentsDesc") }}
                </span>
            </span>
        </div>

        <!-- Variables -->
        <div
            class="robot-section-card"
            @click="
                router.push({ name: 'variables', params: { robotId: robotId } })
            "
        >
            <span class="section-icon sky"><SolarDownloadOutline /></span>
            <span class="section-body">
                <span class="section-head">
                    {{ $t("guide.title3") }}
                    <el-icon><EpArrowRightBold /></el-icon>
                </span>
                <span class="section-desc">{{ $t("guide.desc3") }}</span>
            </span>
        </div>

        <!-- External APIs -->
        <div
            class="robot-section-card"
            @click="
                router.push({
                    name: 'externalHttpApis',
                    params: { robotId: robotId },
                })
            "
        >
            <span class="section-icon rose"><SolarRouting2Linear /></span>
            <span class="section-body">
                <span class="section-head">
                    {{ $t("guide.eApiTitle") }}
                    <el-icon><EpArrowRightBold /></el-icon>
                </span>
                <span class="section-desc">{{ $t("guide.eApiDesc") }}</span>
            </span>
        </div>

        <!-- Settings -->
        <div
            class="robot-section-card"
            @click="
                router.push({ name: 'settings', params: { robotId: robotId } })
            "
        >
            <span class="section-icon indigo"><EpSetting /></span>
            <span class="section-body">
                <span class="section-head">
                    {{ $t("guide.title4") }}
                    <el-icon><EpArrowRightBold /></el-icon>
                </span>
                <span class="section-desc">{{ $t("guide.desc4") }}</span>
            </span>
        </div>

        <!-- Docs (external) -->
        <a
            href="https://braidwork.github.io/kiko/doc"
            target="_blank"
            class="robot-section-card"
        >
            <span class="section-icon sky"><SolarDocumentTextLinear /></span>
            <span class="section-body">
                <span class="section-head">
                    {{ $t("guide.title5") }}
                    <el-icon><BiBoxArrowUpRight /></el-icon>
                </span>
                <span class="section-desc">{{ $t("guide.desc5") }}</span>
            </span>
        </a>
    </div>

    <el-dialog v-model="dialogFormVisible" :title="t('guide.chRoNaBtn')">
        <el-form :model="robotData">
            <el-form-item
                :label="t('common.name')"
                :label-width="formLabelWidth"
            >
                <el-input v-model="robotData.robotName" autocomplete="off" />
            </el-form-item>
        </el-form>
        <template #footer>
            <span class="dialog-footer">
                <el-button @click="
                    robotData.robotName = robotNameForRestore;
                    dialogFormVisible = false;
                ">{{ $t("common.cancel") }}</el-button>
                <el-button
                    type="primary"
                    @click="
                        dialogFormVisible = false;
                        updateRobot();
                    "
                >
                    {{ $t("common.save") }}
                </el-button>
            </span>
        </template>
    </el-dialog>
</template>
