import { createRouter, createWebHistory } from 'vue-router'

const routes = [
  {
    path: '/',
    redirect: '/connections'
  },
  {
    path: '/connections',
    name: 'Connections',
    component: () => import('../views/Connections.vue')
  },
  {
    path: '/manage/:id',
    name: 'Manage',
    component: () => import('../views/Manage.vue')
  }
]

const router = createRouter({
  history: createWebHistory(),
  routes
})

export default router
