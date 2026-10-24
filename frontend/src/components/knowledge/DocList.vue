<script setup>
import { reactive, ref, onMounted, nextTick } from "vue";
import { useRoute, useRouter } from "vue-router";
import { httpReq } from "../../assets/tools.js";
import { UploadFilled } from "@element-plus/icons-vue";
import { useI18n } from "vue-i18n";
const route = useRoute();
const router = useRouter();
const { t, tm, rt } = useI18n();
import MaterialSymbolsBook5Outline from "~icons/material-symbols/book-5-outline";
import EpPlus from "~icons/ep/plus";
const robotId = route.params.robotId;
// const uploadUrlHost = window.location.href.indexOf('localhost') > -1 ? 'http://localhost:12715' : '';
const uploadUrl =
    import.meta.env.VITE_REQ_BACKEND_PREFIX +
    "kb/doc/upload?robotId=" +
    robotId;
const formLabelWidth = "80px";
const docDetailVisible = ref(false);
const editFormVisible = ref(false);
const loading = ref(false);
const docFile = reactive({
    id: 0,
    fileName: "",
    fileSize: 0,
    docContent: "",
    fileSizeInBytes: 0,
});
const tableData = reactive([]);
const listColumnNames = tm("kb.doc.listColumnNames");
/*
const updateUploadingProgress = (evt, uploadFile, uploadFiles) => {
    // const file = document.getElementById('f').files[0];
    // fetch('', {
    //     method: 'POST',
    //     body: file,
    //     onprogress: (event) => {
    //         if (event.lengthComputable) {
    //             const p = Math.round(event.loaded / event.total * 100);
    //         }
    //     },
    // });
    if (evt.lengthComputable) {
        const p = `Uploading (${((evt.loaded / evt.total) * 100).toFixed(2,)}%)…`;
        const m = 'Uploading ' + JSON.stringify(uploadFile) + ' ' + p + '%'
        console.log(m)
    }
}
*/
const listDocs = async () => {
    const t = await httpReq("GET", "kb/doc", { robotId: robotId }, null, null);
    console.log(t);
    if (t.status == 200) tableData.splice(0, tableData.length, ...t.data);
};
onMounted(() => {
    listDocs();
});
const uploadSuccessful = (res, uploadFile, uploadFiles) => {
    console.log(uploadFile);
    listDocs();
};
const uploadFailed = (err, uploadFile, uploadFiles) => {
    console.log(err);
};
const showDocDetail = (idx) => {
    docFile.fileName = tableData[idx].fileName;
    docFile.docContent = tableData[idx].docContent;
    docDetailVisible.value = true;
};
const editDoc = (idx) => {
    docFile.id = tableData[idx].id;
    docFile.fileName = tableData[idx].fileName;
    docFile.docContent = tableData[idx].docContent;
    editFormVisible.value = true;
};
const updateDocContent = () => {
    const t = httpReq("POST", "kb/doc", { robotId: robotId }, null, docFile)
        .then((res) => console.log(res))
        .then(() => {
            editFormVisible.value = false;
            listDocs();
        });
};
const deleteDoc = (idx) => {
    ElMessageBox.confirm("Confirm to delete this document?", "Warning", {
        confirmButtonText: t("common.del"),
        cancelButtonText: t("common.cancel"),
        type: "warning",
    })
        .then(async () => {
            const d = tableData[idx];
            console.log(idx, d, d.id);
            if (d) {
                const t = await httpReq(
                    "DELETE",
                    "kb/doc",
                    { robotId: robotId },
                    null,
                    { id: d.id, fileName: "", fileSize: 0, docContent: "" },
                );
                console.log(t);
                nextTick(() => {
                    listDocs();
                });
            }
        })
        .catch((e) => {
            console.log(e);
            ElMessage({
                type: "info",
                message: "Delete canceled",
            });
        });
};
const goBack = () => {
    router.push({ name: "robotDetail", params: { robotId: robotId } });
};
</script>
<template>
    <div class="page-header">
        <h1 class="page-title">
            <span class="page-title-icon"><MaterialSymbolsBook5Outline /></span>
            {{ t("kb.doc.title") }}
        </h1>
        <div class="page-actions">
            <el-button type="primary" @click="dryRunFormVisible = true"
                >Test docs</el-button
            >
        </div>
    </div>
    <div class="page-card">
        <el-upload
            drag
            class="doc-upload"
            :action="uploadUrl"
            :on-success="uploadSuccessful"
            :on-error="uploadFailed"
        >
            <el-icon class="el-icon--upload"><upload-filled /></el-icon>
            <div class="el-upload__text" v-html="t('kb.doc.uploadBoxTip')"></div>
            <template #tip>
                <div class="el-upload__tip">
                    {{ t("kb.doc.uploadFileSizeLimitTip") }}
                </div>
            </template>
        </el-upload>
        <el-table :data="tableData" stripe style="width: 100%">
            <el-table-column
                prop="fileName"
                :label="listColumnNames[0]"
                width="200"
            />
            <el-table-column
                prop="fileSize"
                :label="listColumnNames[1]"
                width="150"
            />
            <el-table-column prop="docContent" :label="listColumnNames[2]">
                <template #default="scope">
                    <el-text truncated>{{ scope.row.docContent }}</el-text>
                </template>
            </el-table-column>
            <el-table-column
                fixed="right"
                :label="listColumnNames[3]"
                width="220"
                align="center"
            >
                <template #default="scope">
                    <el-button
                        link
                        type="primary"
                        @click="showDocDetail(scope.$index)"
                        >{{ t("common.toDetail") }}</el-button
                    >
                    <el-button
                        link
                        type="primary"
                        @click="editDoc(scope.$index)"
                        >{{ t("common.edit") }}</el-button
                    >
                    <el-button
                        link
                        type="danger"
                        @click="deleteDoc(scope.$index)"
                        >{{ t("common.del") }}</el-button
                    >
                </template>
            </el-table-column>
        </el-table>
    </div>
    <el-dialog
        v-model="docDetailVisible"
        :title="docFile.fileName"
        width="800"
    >
        <div style="white-space: pre-wrap; line-height: 1.8">
            {{ docFile.docContent }}
        </div>
        <template #footer>
            <div class="dialog-footer">
                <el-button @click="docDetailVisible = false">Close</el-button>
            </div>
        </template>
    </el-dialog>
    <el-drawer
        v-model="editFormVisible"
        :title="docFile.fileName"
        direction="rtl"
        size="70%"
        :append-to-body="true"
        :destroy-on-close="true"
    >
        <el-form :model="docFile">
            <el-form-item label="Content" :label-width="formLabelWidth">
                <el-input
                    v-model="docFile.docContent"
                    placeholder=""
                    type="textarea"
                    :rows="30"
                />
            </el-form-item>
        </el-form>
        <div class="demo-drawer__footer">
            <el-button
                type="primary"
                :loading="loading"
                @click="updateDocContent()"
                >{{ t("common.save") }}</el-button
            >
            <el-button @click="editFormVisible = false">{{
                t("common.cancel")
            }}</el-button>
        </div>
    </el-drawer>
</template>
<style scoped>
.doc-upload :deep(.el-upload-dragger) {
    border-radius: 12px;
    border-color: #d3dce6;
    transition: border-color 0.2s ease;
}

.doc-upload :deep(.el-upload-dragger:hover) {
    border-color: #6366f1;
    background: #f7f7ff;
}
</style>
