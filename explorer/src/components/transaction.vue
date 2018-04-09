<template>
  <div>
    <h3>Show Transaction</h3>
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
