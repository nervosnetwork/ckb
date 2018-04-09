import Vue from 'vue'
import Moment from 'vue-moment'
import App from './components/app.vue'
import router from './router'

Vue.use(Moment)

new Vue({
  router,
  el: '#app',
  render: h => h(App)
})
