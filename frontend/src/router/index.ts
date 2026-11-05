import { createRouter, createWebHistory } from 'vue-router'
import { tokens } from '@/api/client'
import AppShell from '@/layouts/AppShell.vue'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/login',
      name: 'login',
      component: () => import('@/views/LoginView.vue'),
      meta: { public: true },
    },
    {
      path: '/',
      component: AppShell,
      children: [
        { path: '', redirect: { name: 'instances' } },
        {
          path: 'instances',
          name: 'instances',
          component: () => import('@/views/InstancesView.vue'),
        },
        {
          path: 'instances/new',
          name: 'instance-new',
          component: () => import('@/views/InstanceWizardView.vue'),
        },
        {
          path: 'instances/:id',
          name: 'instance',
          component: () => import('@/views/InstanceDetailView.vue'),
          props: true,
        },
        {
          path: 'audit',
          name: 'audit',
          component: () => import('@/views/AuditView.vue'),
        },
        {
          path: 'settings',
          name: 'settings',
          component: () => import('@/views/SettingsView.vue'),
        },
      ],
    },
    { path: '/:pathMatch(.*)*', redirect: { name: 'instances' } },
  ],
})

/** 全局守卫：未登录跳登录页，并记住原目标。 */
router.beforeEach((to) => {
  if (to.meta.public) {
    // 已登录时不再回到登录页
    if (tokens.has && to.name === 'login') return { name: 'instances' }
    return true
  }
  if (!tokens.has) {
    return { name: 'login', query: { redirect: to.fullPath } }
  }
  return true
})

export default router
