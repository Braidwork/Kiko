<script setup>
import { ref, reactive, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { copyProperties, httpReq } from "../assets/tools.js";
import EpArrowRightBold from "~icons/ep/arrow-right-bold";
import EpSetting from "~icons/ep/setting";
import SolarDocumentTextLinear from "~icons/solar/document-text-linear";
import BiBoxArrowUpRight from "~icons/bi/box-arrow-up-right";
import RiRobot2Line from "~icons/ri/robot-2-line";
import EpPlus from "~icons/ep/plus";
import EpRefresh from "~icons/ep/refresh";
import EpPromotion from "~icons/ep/promotion";
import OutboundCallBotAvatar from "@/assets/outbound-bot.png";
import InboundCallBotAvatar from "@/assets/inbound-bot.png";
import TextBotAvatar from "@/assets/text-bot.png";
import LanguageSwitcher from "./LanguageSwitcher.vue";
const { t, locale } = useI18n();
const isZhLang = locale.value == "zh";
const router = useRouter();
const currentVersion = ref("");
const checkUpdateResult = ref(0);
const updateLoading = ref(false);
const newVersion = ref("");
const changelog = reactive([]);
const setFormVisible = ref(false);
const formLabelWidth = "90px";
const checkUpdate = async () => {
    updateLoading.value = true;
    const t = await httpReq("GET", "check-new-version.json", null, null, null);
    if (t.status == 200) {
        if (t.data != null) {
            newVersion.value = t.data.version;
            changelog.splice(0, changelog.length);
            copyProperties(t.data.changelog, changelog);
            checkUpdateResult.value = 1;
        } else {
            checkUpdateResult.value = 2;
        }
    } else {
        checkUpdateResult.value = 3;
    }
    updateLoading.value = false;
};
const toSettings = () => {
    router.push("/settings");
};

const robots = reactive([]);
const robotData = reactive({
    robotId: "",
    robotName: "",
    robotType: "",
});
onMounted(async () => {
    await list();
    const t = await httpReq("GET", "version.json", null, null, null);
    currentVersion.value = t;
});

async function list() {
    const t = await httpReq("GET", "robot", null, null, null);
    if (t.status == 200) {
        robots.splice(0, robots.length, ...t.data.reverse());
    }
}

async function newRobot() {
    const t = await httpReq("POST", "robot", null, null, robotData);
    if (t.status == 200) await list();
    setFormVisible.value = false;
}
function showRobotForm() {
    robotData.robotId = "";
    robotData.robotName = "";
    robotData.robotType = "";
    setFormVisible.value = true;
}
function robotDetail(id, name) {
    router.push({ name: "robotDetail", params: { robotId: id } });
}
const getBotAvatar = (type) => {
    if (type == "OutboundCallBot") return OutboundCallBotAvatar;
    else if (type == "InboundCallBot") return InboundCallBotAvatar;
    else if (type == "TextBot") return TextBotAvatar;
    else return "";
};
const getBotType = (type) => {
    if (type == "OutboundCallBot")
        return isZhLang ? "语音外呼机器人" : "Outbound call bot";
    else if (type == "InboundCallBot")
        return isZhLang ? "语音呼入机器人" : "Incoming call bot";
    else if (type == "TextBot")
        return isZhLang ? "文本机器人" : "Text chat bot";
    else return "";
};
const compareDifferentRobotTypeData = [
    {
        rtype: getBotType("OutboundCallBot"),
        dialogNodeAnswerTextType: isZhLang
            ? "普通文本, 非流式响应"
            : "Plain text, No streaming response",
        llmChatNodeAsyncResponse: isZhLang
            ? "非流式响应"
            : "No streaming response",
    },
    {
        rtype: getBotType("InboundCallBot"),
        dialogNodeAnswerTextType: isZhLang
            ? "普通文本, 非流式响应"
            : "Plain text, No streaming response",
        llmChatNodeAsyncResponse: isZhLang
            ? "非流式响应"
            : "No streaming response",
    },
    {
        rtype: getBotType("TextBot"),
        dialogNodeAnswerTextType: isZhLang
            ? "富文本, 流式响应"
            : "Rich text, Streaming response",
        llmChatNodeAsyncResponse: isZhLang ? "流式响应" : "Streaming response",
    },
];
</script>
<style scoped>
.page {
    min-height: 100vh;
    background: #f6f8fb;
    display: flex;
    flex-direction: column;
}

/* ===== Top bar ===== */
.topbar {
    position: sticky;
    top: 0;
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 40px;
    background: rgba(255, 255, 255, 0.85);
    backdrop-filter: blur(12px);
    border-bottom: 1px solid #eef1f6;
}

.brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 20px;
    font-weight: 700;
    color: #1f2d3d;
}

.brand-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    background: linear-gradient(135deg, #6366f1, #8b5cf6);
    color: #fff;
    font-size: 20px;
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.35);
}

.topbar-actions {
    display: flex;
    align-items: center;
    gap: 10px;
}

.icon-btn {
    width: 38px;
    height: 38px;
    border-radius: 10px;
    border: 1px solid #e5e9f2;
    background: #fff;
    color: #4e5969;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 18px;
    cursor: pointer;
    transition: all 0.2s ease;
}

.icon-btn:hover {
    color: #6366f1;
    border-color: #c7d2fe;
    background: #f5f6ff;
    transform: translateY(-1px);
}

.icon-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
}

.new-version-btn {
    border-radius: 8px;
}

/* ===== Hero ===== */
.hero {
    margin: 32px 40px 0;
    padding: 44px 48px;
    border-radius: 20px;
    background:
        radial-gradient(circle at 85% 20%, rgba(139, 92, 246, 0.35), transparent 55%),
        radial-gradient(circle at 15% 90%, rgba(56, 189, 248, 0.3), transparent 55%),
        linear-gradient(135deg, #4f46e5, #7c3aed);
    color: #fff;
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 24px;
    box-shadow: 0 12px 32px rgba(79, 70, 229, 0.25);
    overflow: hidden;
    position: relative;
}

.hero-title {
    display: flex;
    align-items: center;
    gap: 14px;
    font-size: 32px;
    font-weight: 700;
    line-height: 1.2;
}

.hero-desc {
    margin-top: 10px;
    font-size: 15px;
    opacity: 0.85;
    max-width: 560px;
}

.create-btn {
    border: none;
    font-weight: 600;
    border-radius: 10px;
    padding: 20px 26px;
    font-size: 15px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
}

/* ===== Robot cards ===== */
.section {
    padding: 0 40px;
    margin-top: 36px;
}

.section-title {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 20px;
    font-weight: 700;
    color: #1f2d3d;
    margin-bottom: 20px;
}

.robot-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 20px;
}

.robot-card {
    position: relative;
    background: #fff;
    border: 1px solid #eef1f6;
    border-radius: 16px;
    padding: 28px 20px 22px;
    text-align: center;
    cursor: pointer;
    transition:
        transform 0.25s ease,
        box-shadow 0.25s ease,
        border-color 0.25s ease;
}

.robot-card:hover {
    transform: translateY(-4px);
    border-color: transparent;
    box-shadow: 0 14px 32px rgba(31, 45, 61, 0.12);
}

.robot-card::before {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: 16px;
    padding: 1.5px;
    background: linear-gradient(135deg, #6366f1, #8b5cf6);
    -webkit-mask:
        linear-gradient(#fff 0 0) content-box,
        linear-gradient(#fff 0 0);
    -webkit-mask-composite: xor;
    mask-composite: exclude;
    opacity: 0;
    transition: opacity 0.25s ease;
    pointer-events: none;
}

.robot-card:hover::before {
    opacity: 1;
}

.robot-avatar {
    width: 84px;
    height: 84px;
    object-fit: contain;
    margin-bottom: 14px;
    transition: transform 0.25s ease;
}

.robot-card:hover .robot-avatar {
    transform: scale(1.06);
}

.robot-name {
    font-size: 17px;
    font-weight: 600;
    color: #1f2d3d;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.robot-type {
    display: inline-block;
    margin-top: 8px;
    padding: 3px 12px;
    font-size: 12px;
    border-radius: 999px;
    color: #6366f1;
    background: #eef2ff;
}

.robot-detail-btn {
    margin-top: 16px;
    border-radius: 8px;
}

/* Empty state */
.empty-state {
    grid-column: 1 / -1;
    background: #fff;
    border: 1px dashed #d3dce6;
    border-radius: 16px;
    padding: 56px 20px;
    text-align: center;
    color: #86909c;
}

.empty-state .el-icon {
    font-size: 44px;
    color: #c9d2e0;
    margin-bottom: 12px;
}

.empty-state p {
    margin: 0 0 16px;
}

/* ===== Feature cards ===== */
.feature-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 20px;
}

.feature-card {
    display: flex;
    align-items: flex-start;
    gap: 16px;
    background: #fff;
    border: 1px solid #eef1f6;
    border-radius: 16px;
    padding: 24px;
    color: inherit;
    text-decoration: none;
    transition:
        transform 0.25s ease,
        box-shadow 0.25s ease;
}

.feature-card:hover {
    transform: translateY(-3px);
    box-shadow: 0 12px 28px rgba(31, 45, 61, 0.1);
}

.feature-icon {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 46px;
    height: 46px;
    border-radius: 12px;
    font-size: 22px;
}

.feature-icon.indigo {
    color: #6366f1;
    background: #eef2ff;
}

.feature-icon.sky {
    color: #0284c7;
    background: #e0f2fe;
}

.feature-body {
    flex: 1;
    min-width: 0;
}

.feature-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 16px;
    font-weight: 600;
    color: #1f2d3d;
}

.feature-title .el-icon {
    font-size: 14px;
    color: #86909c;
    transition:
        transform 0.2s ease,
        color 0.2s ease;
}

.feature-card:hover .feature-title .el-icon {
    transform: translate(2px, -2px);
    color: #6366f1;
}

.feature-desc {
    margin-top: 6px;
    font-size: 13px;
    line-height: 1.7;
    color: #86909c;
}

/* ===== Alerts ===== */
.alerts {
    padding: 12px 40px 0;
}

/* ===== Footer ===== */
.footer {
    margin-top: auto;
    padding: 40px 20px 28px;
    text-align: center;
    font-size: 13px;
    color: #a0a6b1;
    line-height: 2;
}

.footer a {
    color: #6366f1;
    text-decoration: none;
}

.footer a:hover {
    text-decoration: underline;
}

/* ===== Dialog ===== */
.type-table {
    margin-top: 8px;
}

.type-table :deep(.el-table__row) {
    --el-table-tr-hover-bg-color: #f5f6ff;
}

@media (max-width: 768px) {
    .topbar {
        padding: 12px 20px;
    }

    .hero {
        margin: 20px 20px 0;
        padding: 32px 24px;
    }

    .section,
    .alerts {
        padding: 0 20px;
    }

    .hero-title {
        font-size: 24px;
    }
}
</style>
<template>
    <div class="page">
        <!-- ===== Top bar ===== -->
        <header class="topbar">
            <div class="brand">
                <span class="brand-icon">
                    <RiRobot2Line />
                </span>
                {{ t("home.workspace") }}
            </div>
            <div class="topbar-actions">
                <el-tooltip
                    :content="isZhLang ? '检查更新' : 'Check update'"
                    placement="bottom"
                >
                    <button
                        class="icon-btn"
                        :disabled="updateLoading"
                        @click="checkUpdate"
                    >
                        <el-icon :class="{ 'is-loading': updateLoading }">
                            <EpRefresh />
                        </el-icon>
                    </button>
                </el-tooltip>
                <el-tooltip
                    :content="$t('home.globalSettings')"
                    placement="bottom"
                >
                    <button class="icon-btn" @click="toSettings">
                        <el-icon><EpSetting /></el-icon>
                    </button>
                </el-tooltip>
                <LanguageSwitcher />
            </div>
        </header>

        <!-- ===== Update alerts ===== -->
        <div class="alerts">
            <el-popover
                ref="popover"
                placement="bottom-start"
                title="Changelog"
                :width="320"
                trigger="hover"
            >
                <template #reference>
                    <el-button
                        v-show="checkUpdateResult === 1"
                        class="new-version-btn"
                        type="warning"
                        round
                    >
                        Found new version: {{ newVersion }}
                    </el-button>
                </template>
                <template #default>
                    <ol style="margin: 0; padding-left: 18px">
                        <li
                            v-for="(item, index) in changelog"
                            :id="index"
                            :key="index"
                        >
                            {{ item }}
                        </li>
                    </ol>
                    <a
                        href="https://github.com/Braidwork/Kiko/releases"
                        target="_blank"
                        >Go to download</a
                    >
                </template>
            </el-popover>
            <el-alert
                v-show="checkUpdateResult === 2"
                title="You're using the latest version."
                type="success"
                show-icon
                @close="checkUpdateResult = 0"
            />
            <el-alert
                v-show="checkUpdateResult === 3"
                title="Failed to check update information, please try again later."
                type="error"
                show-icon
                @close="checkUpdateResult = 0"
            />
        </div>

        <!-- ===== Hero ===== -->
        <section class="hero">
            <div>
                <div class="hero-title">
                    <el-icon><RiRobot2Line /></el-icon>
                    {{ t("home.robotListTitle") }}
                </div>
                <div class="hero-desc">{{ $t("home.subTitle") }}</div>
            </div>
            <el-button
                class="create-btn"
                size="large"
                type="primary"
                @click="showRobotForm"
            >
                <el-icon style="margin-right: 6px"><EpPlus /></el-icon>
                {{ t("home.createRobotBtnTxt") }}
            </el-button>
        </section>

        <!-- ===== Robot list ===== -->
        <section class="section">
            <div class="robot-grid">
                <div
                    v-for="n in robots"
                    :key="n.robotId"
                    class="robot-card"
                    @click="robotDetail(n.robotId, n.robotName)"
                >
                    <img
                        class="robot-avatar"
                        :src="getBotAvatar(n.robotType)"
                        alt=""
                    />
                    <div class="robot-name" :title="n.robotName">
                        {{ n.robotName }}
                    </div>
                    <span class="robot-type">{{
                        getBotType(n.robotType)
                    }}</span>
                    <div>
                        <el-button
                            class="robot-detail-btn"
                            type="primary"
                            round
                            @click.stop="robotDetail(n.robotId, n.robotName)"
                        >
                            {{ t("common.toDetail") }}
                        </el-button>
                    </div>
                </div>
                <!-- Empty state -->
                <div v-if="robots.length === 0" class="empty-state">
                    <el-icon><RiRobot2Line /></el-icon>
                    <p>{{ t("home.robotListTitle") }}</p>
                    <el-button type="primary" round @click="showRobotForm">
                        <el-icon style="margin-right: 6px"><EpPlus /></el-icon>
                        {{ t("home.createRobotBtnTxt") }}
                    </el-button>
                </div>
            </div>
        </section>

        <!-- ===== Quick links ===== -->
        <section class="section">
            <div class="feature-grid">
                <router-link to="/settings" class="feature-card">
                    <span class="feature-icon indigo">
                        <EpSetting />
                    </span>
                    <span class="feature-body">
                        <span class="feature-title">
                            {{ $t("home.globalSettings") }}
                            <el-icon><EpArrowRightBold /></el-icon>
                        </span>
                        <span class="feature-desc">{{
                            $t("guide.desc4")
                        }}</span>
                    </span>
                </router-link>
                <a
                    href="https://braidwork.github.io/kiko/#/doc"
                    target="_blank"
                    class="feature-card"
                >
                    <span class="feature-icon sky">
                        <SolarDocumentTextLinear />
                    </span>
                    <span class="feature-body">
                        <span class="feature-title">
                            {{ $t("guide.title5") }}
                            <el-icon><BiBoxArrowUpRight /></el-icon>
                        </span>
                        <span class="feature-desc">{{
                            $t("guide.desc5")
                        }}</span>
                    </span>
                </a>
            </div>
        </section>

        <!-- ===== Footer ===== -->
        <footer class="footer">
            <div>
                Version: {{ currentVersion }} ·
                <a href="https://braidwork.github.io/kiko/" target="_blank"
                    >braidwork.github.io/kiko</a
                >
            </div>
            <div>
                If you have any questions or suggestions, please create a
                <a
                    href="https://github.com/Braidwork/Kiko/discussions"
                    target="_blank"
                    >discussion</a
                >
                on Github or email to: dialogflow@yeah.net
            </div>
            <div>Some icons were created by <a href="https://www.flaticon.com/" target="_blank">Flaticon</a></div>
        </footer>

        <!-- ===== Create robot dialog ===== -->
        <el-dialog
            v-model="setFormVisible"
            :title="t('home.createRobotBtnTxt')"
            width="640px"
            destroy-on-close
        >
            <el-form :model="robotData">
                <el-form-item
                    :label="t('common.name')"
                    :label-width="formLabelWidth"
                    prop="robotName"
                    :rules="[
                        { required: true, message: 'Robot name is required' },
                    ]"
                >
                    <el-input
                        v-model="robotData.robotName"
                        autocomplete="off"
                        :placeholder="t('common.name')"
                    />
                </el-form-item>
                <el-form-item
                    :label="t('common.type')"
                    :label-width="formLabelWidth"
                    prop="robotType"
                    :rules="[
                        {
                            required: true,
                            message: 'Please choose a type of robot',
                        },
                    ]"
                >
                    <el-select
                        v-model="robotData.robotType"
                        :placeholder="isZhLang ? '请选择机器人类型' : 'Select robot type'"
                        style="width: 100%"
                    >
                        <el-option
                            :label="getBotType('TextBot')"
                            value="TextBot"
                        />
                        <el-option
                            :label="getBotType('InboundCallBot')"
                            value="InboundCallBot"
                        />
                        <el-option
                            :label="getBotType('OutboundCallBot')"
                            value="OutboundCallBot"
                        />
                    </el-select>
                </el-form-item>
            </el-form>
            <el-table
                class="type-table"
                :data="compareDifferentRobotTypeData"
                size="small"
                border
            >
                <el-table-column property="rtype" label="" width="180" />
                <el-table-column
                    property="dialogNodeAnswerTextType"
                    :label="isZhLang ? '话术节点' : 'Dialog node'"
                />
                <el-table-column
                    property="llmChatNodeAsyncResponse"
                    :label="
                        isZhLang ? '大模型聊天节点' : 'Llm chat node streaming'
                    "
                />
            </el-table>
            <template #footer>
                <el-button @click="setFormVisible = false">{{
                    $t("common.cancel")
                }}</el-button>
                <el-button type="primary" @click="newRobot()">{{
                    $t("common.create")
                }}</el-button>
            </template>
        </el-dialog>
    </div>
</template>
