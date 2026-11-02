<script setup>
import { ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';
import { changePassword, logout } from '../../assets/auth.js';

const { t } = useI18n();
const router = useRouter();
const oldPassword = ref('');
const newPassword = ref('');
const confirmPassword = ref('');
const loading = ref(false);
const error = ref('');

async function submit() {
    error.value = '';
    if (!oldPassword.value || !newPassword.value) {
        error.value = t('auth.required');
        return;
    }
    if (newPassword.value !== confirmPassword.value) {
        error.value = t('auth.mismatch');
        return;
    }
    if (newPassword.value.length < 12) {
        error.value = t('auth.passwordHint');
        return;
    }
    loading.value = true;
    try {
        await changePassword(oldPassword.value, newPassword.value);
        await router.replace('/');
    } catch (e) {
        error.value = e?.message || t('auth.changeFailed');
    } finally {
        loading.value = false;
    }
}

async function signOut() {
    await logout();
    await router.replace('/login');
}
</script>

<template>
    <div class="auth-page">
        <div class="auth-card">
            <div class="auth-brand">
                <span class="auth-logo">K</span>
                <span>Kiko</span>
            </div>
            <h1 class="auth-title">{{ t('auth.changeTitle') }}</h1>
            <p class="auth-subtitle">{{ t('auth.changeSubtitle') }}</p>

            <el-form class="auth-form" @submit.prevent="submit">
                <el-form-item>
                    <el-input v-model="oldPassword" size="large" type="password" show-password
                        autocomplete="current-password" :placeholder="t('auth.oldPassword')"
                        @keyup.enter="submit" />
                </el-form-item>
                <el-form-item>
                    <el-input v-model="newPassword" size="large" type="password" show-password
                        autocomplete="new-password" :placeholder="t('auth.newPassword')"
                        @keyup.enter="submit" />
                </el-form-item>
                <el-form-item>
                    <el-input v-model="confirmPassword" size="large" type="password" show-password
                        autocomplete="new-password" :placeholder="t('auth.confirmPassword')"
                        @keyup.enter="submit" />
                </el-form-item>
            </el-form>

            <el-alert v-if="error" class="auth-error" :title="error" type="error" :closable="false" show-icon />
            <el-button class="auth-submit" type="primary" size="large" :loading="loading" @click="submit">
                {{ loading ? t('auth.changing') : t('auth.change') }}
            </el-button>
            <el-button class="auth-signout" link @click="signOut">{{ t('auth.logout') }}</el-button>
        </div>
    </div>
</template>

<style scoped>
.auth-page {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background:
        radial-gradient(circle at 85% 15%, rgba(139, 92, 246, 0.28), transparent 55%),
        radial-gradient(circle at 10% 90%, rgba(56, 189, 248, 0.22), transparent 55%),
        linear-gradient(135deg, #eef2ff, #f8fafc 55%, #eef2ff);
}

.auth-card {
    width: 100%;
    max-width: 400px;
    padding: 40px 36px 32px;
    border-radius: 20px;
    background: rgba(255, 255, 255, 0.92);
    box-shadow: 0 24px 60px rgba(31, 45, 61, 0.16);
    backdrop-filter: blur(14px);
}

.auth-brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 700;
    font-size: 18px;
    color: #1f2d3d;
}

.auth-logo {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 10px;
    background: linear-gradient(135deg, #6366f1, #8b5cf6);
    color: #fff;
    box-shadow: 0 4px 12px rgba(99, 102, 241, 0.4);
}

.auth-title {
    margin: 26px 0 6px;
    font-size: 22px;
    color: #1f2d3d;
}

.auth-subtitle {
    margin: 0 0 22px;
    font-size: 13px;
    color: #86909c;
    line-height: 1.7;
}

.auth-form :deep(.el-form-item) {
    margin-bottom: 16px;
}

.auth-error {
    margin-bottom: 14px;
}

.auth-submit {
    width: 100%;
    height: 40px;
    border: none;
    font-weight: 600;
    border-radius: 10px;
}

.auth-signout {
    width: 100%;
    height: 40px;
    /* Element Plus 会给相邻的 el-button 加 margin-left: 12px；两个按钮都是
       100% 宽时会把第二个按钮整体推歪，文字看起来没有对齐。 */
    margin: 14px 0 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #86909c;
}
</style>
