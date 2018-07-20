<template>
  <div>
    <h3>Show Transaction</h3>
    <div>
      Inputs
      <ul class="uk-list">
        <li v-for="i in transaction.inputs" :key="i">
          {{ i }}
        </li>
      </ul>
      Outputs
      <ul class="uk-list">
        <li v-for="o in transaction.outputs" :key="o">
          {{ o }}
        </li>
      </ul>
    </div>
  </div>
</template>

<script>
export default {
  props: ['transaction', 'get_transaction'],

  created () {
    this.get_transaction(this.$route.params.id)
  }, 

  beforeRouteUpdate (to, from, next) {
    this.get_transaction(to.params.id)
  }, 
}
</script>
