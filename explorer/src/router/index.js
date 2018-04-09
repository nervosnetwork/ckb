import Vue from 'vue'
import Router from 'vue-router'
import Home from '../components/home.vue'
import Block from '../components/block.vue'
import Transaction from '../components/transaction.vue'
import SendTransaction from '../components/send_transaction.vue'

Vue.use(Router)

export default new Router({
    mode: 'history',
    base: __dirname,
    routes: [
      { name: 'home', path: '/', component: Home },
      { name: 'send_transaction', path: '/send_transaction', component: SendTransaction },
      { name: 'blocks', path: '/blocks/:id', component: Block },
      { name: 'transactions', path: '/transactions/:id', component: Transaction }
    ]
})
