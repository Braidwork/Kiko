<script setup>
import { ref, reactive, onMounted } from 'vue';
import { useRoute,useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n'
// import { ElMessage, ElMessageBox } from 'element-plus'
const { t, tm, rt } = useI18n();
import { btoa, httpReq } from '../../assets/tools.js'
import SolarRouting2Linear from '~icons/solar/routing-2-linear'
import EpPlus from '~icons/ep/plus'
const route=useRoute();
const router = useRouter();
const robotId=route.params.robotId

const tableData = ref([])
onMounted(async () => {
    const t = await httpReq('GET', 'external/http', {robotId:robotId}, null, null);
    // console.log(t);
    if (t && t.status == 200) {
        tableData.value = t.data == null ? [] : t.data;
    }
});

const goBack = () => {
    router.push({ name: 'robotDetail', params: { robotId: robotId } });
}
const newApi = () => {
    router.push({ name: 'externalHttpApiDetail', params: { id: 'new' } })
}
const editApi = (idx, row) => {
    router.push({ name: 'externalHttpApiDetail', params: { id: row.id } })
}
const delApi = (idx, row) => {
    ElMessageBox.confirm(
        'Confirm whether to permanently delete this record?',
        'Warning',
        {
            confirmButtonText: 'OK',
            cancelButtonText: 'Cancel',
            type: 'warning',
        }
    )
        .then(async () => {
            const t = await httpReq('DELETE', 'external/http/' + row.id, { robotId: robotId }, null, null);
            // console.log(t);
            if (t && t.status == 200) {
                ElMessage({
                    showClose: true,
                    message: 'Successfully deleted.',
                    type: 'success',
                });
                tableData.value.splice(idx, 1);
            } else {
                ElMessage({
                    showClose: true,
                    message: 'Delete failed.',
                    type: 'error',
                })
            }
        })
        .catch(() => {
        })
}
</script>
<template>
    <div class="page-header">
        <h1 class="page-title">
            <span class="page-title-icon"><SolarRouting2Linear /></span>
            {{ t('eApi.title') }}
        </h1>
        <div class="page-actions">
            <el-button type="primary" @click="newApi()">
                <el-icon style="margin-right: 6px"><EpPlus /></el-icon>
                {{ t('eApi.add') }}
            </el-button>
        </div>
    </div>
    <el-alert type="warning" :closable="false" show-icon style="margin-bottom: 16px;">
        <template #title>
            Now you can not only send data to the outside, but also get data from the outside and save it in variables
            by setting value source to a HTTP API.
            <router-link :to="{ name: 'variables', params: { robotId: robotId } }">{{ t('var.add') }}</router-link>
        </template>
    </el-alert>
    <div class="page-card">
        <el-table :data="tableData" stripe style="width: 100%">
            <el-table-column prop="name" :label="t('common.name')" min-width="240" />
            <el-table-column prop="description" :label="t('common.desc')" min-width="300" />
            <el-table-column fixed="right" :label="tm('mainflow.table')[2]" width="160" align="center">
                <template #default="scope">
                    <el-button link type="primary" @click="editApi(scope.$index, scope.row)">
                        Edit
                    </el-button>
                    <el-button link type="danger" @click="delApi(scope.$index, scope.row)">
                        Delete
                    </el-button>
                </template>
            </el-table-column>
        </el-table>
    </div>
</template>