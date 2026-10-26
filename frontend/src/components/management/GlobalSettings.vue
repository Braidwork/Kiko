<script setup>
import { ref, reactive, onMounted } from "vue";
import { useRouter } from "vue-router";
import { copyProperties, httpReq } from "../../assets/tools.js";
import { useI18n } from "vue-i18n";
const { t } = useI18n();
const router = useRouter();

const goBack = () => {
    router.push("/");
};

const settings = reactive({
    ip: "",
    port: 12715,
    selectRandomPortWhenConflict: false,
    hfModelDownload: {
        connectTimeoutMillis: 1000,
        readTimeoutMillis: 10000,
        accessToken: "",
    },
});
const formLabelWidth = "160px";
const saveFailed = ref(false);
const saveFailedDetail = ref("");

onMounted(async () => {
    const r = await httpReq("GET", "management/global-settings", null, null, null);
    if (r.status == 200) {
        copyProperties(r.data, settings);
    }
});

async function save() {
    const r = await httpReq("POST", "management/global-settings", null, null, settings);
    if (r.status == 200) {
        ElMessage({ type: "success", message: t("common.saved") });
        saveFailed.value = false;
    } else {
        const m = t(r.err.message);
        ElMessage.error(m ? m : r.err.message);
    }
}
</script>
<template>
    <div class="page-header">
        <h1 class="page-title">{{ $t("home.globalSettings") }}</h1>
        <el-button @click="goBack()">{{ $t("common.back") }}</el-button>
    </div>

    <el-card class="settings-card" shadow="never">
        <template #header>
            <div class="section-title">{{ $t("settings.commonSettings") }}</div>
        </template>
        <el-form :model="settings" :label-width="formLabelWidth">
            <el-form-item :label="$t('settings.listeningIp')">
                <el-input v-model="settings.ip" />
            </el-form-item>
            <el-form-item label="" :label-width="formLabelWidth">
                <div class="form-item-help">{{ $t("settings.ipNote") }}</div>
            </el-form-item>
            <el-form-item label="" :label-width="formLabelWidth">
                <div class="form-item-help">{{ $t("settings.ipNote2") }}</div>
            </el-form-item>
            <el-form-item :label="$t('settings.prompt2')">
                <el-input-number v-model="settings.port" :min="1024" :max="65530" />
            </el-form-item>
            <el-form-item label="" :label-width="formLabelWidth">
                <div class="switch-row">
                    <el-switch v-model="settings.selectRandomPortWhenConflict" />
                    <span>{{ $t("settings.prompt2_2") }}</span>
                </div>
            </el-form-item>
            <el-form-item label="" :label-width="formLabelWidth">
                <el-alert
                    :title="$t('settings.note')"
                    type="warning"
                    :closable="false"
                    class="note-alert"
                />
            </el-form-item>
        </el-form>
    </el-card>

    <el-card class="settings-card" shadow="never">
        <template #header>
            <div class="section-title">
                {{ $t("settings.hfDownload") }}
                <el-tooltip effect="light" placement="right">
                    <template #content>
                        {{ $t("settings.hfDownloadTip") }}
                    </template>
                    <el-button circle>?</el-button>
                </el-tooltip>
            </div>
        </template>
        <el-form
            :model="settings.hfModelDownload"
            :label-width="formLabelWidth"
        >
            <el-form-item :label="$t('botSettings.connTimeout')">
                <el-input-number
                    v-model="settings.hfModelDownload.connectTimeoutMillis"
                    :min="100"
                    :max="50000"
                    :step="100"
                />
                <span class="form-item-suffix">{{ t("common.millis") }}</span>
            </el-form-item>
            <el-form-item :label="$t('botSettings.readTimeout')">
                <el-input-number
                    v-model="settings.hfModelDownload.readTimeoutMillis"
                    :min="1000"
                    :max="65530"
                    :step="100"
                />
                <span class="form-item-suffix">{{ t("common.millis") }}</span>
            </el-form-item>
            <el-form-item :label="$t('settings.hfAccessToken')">
                <el-input
                    v-model="settings.hfModelDownload.accessToken"
                    type="password"
                    show-password
                />
                <div class="form-item-help">
                    {{ $t("settings.hfAccessTokenHelp") }}
                </div>
            </el-form-item>
        </el-form>
    </el-card>

    <div class="settings-footer">
        <el-button type="primary" @click="save">
            {{ $t("common.save") }}
        </el-button>
        <el-button @click="goBack()">{{ $t("common.back") }}</el-button>
    </div>
</template>
<style scoped>
.settings-card {
    border-radius: 12px;
    margin-bottom: 20px;
}

.settings-card :deep(.el-card__header) {
    padding: 14px 20px 0;
    border-bottom: none;
}

.settings-card .section-title {
    margin: 0;
}

.form-item-help {
    width: 100%;
    color: var(--el-text-color-secondary, #909399);
    font-size: 12px;
    line-height: 1.5;
}

.form-item-suffix {
    margin-left: 8px;
    color: var(--el-text-color-secondary, #909399);
}

.switch-row {
    display: flex;
    align-items: center;
    gap: 10px;
}

.note-alert {
    width: 100%;
}

.settings-footer {
    position: sticky;
    bottom: 0;
    z-index: 10;
    display: flex;
    gap: 4px;
    padding: 12px 0;
    background: var(--el-bg-color, #fff);
    border-top: 1px solid var(--el-border-color-light, #e4e7ed);
}
</style>
