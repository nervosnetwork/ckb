<template>
  <div class="uk-container">
    <div class="uk-grid">
      <div class="uk-width-1-2 uk-card uk-card-default uk-card-body">
        <h3 class="uk-card-title">Blocks</h3>
        <ul class="uk-list">
          <li class="uk-flex" v-for="block in orderedBlocks" :key="block.header.hash">
            <div class="uk-card uk-card-default uk-card-body uk-width-1-2">
              Block #{{ block.header.raw.number }}
              <div class="uk-text-truncate">
                <router-link v-bind:to="{ name: 'blocks', params: { id: block.hash }}">{{ block.hash }}</router-link>
              </div>
            </div>
            <div class="uk-card uk-card-default uk-card-body uk-margin-left uk-width-1-2">
              {{ block.transactions.length }} txns
              <br/>
              {{ new Date(block.header.raw.timestamp) | moment("YYYY-MM-DD HH:mm:ss") }}
            </div>
          </li>
        </ul>
      </div>

      <div class="uk-width-1-2 uk-card uk-card-default uk-card-body">
        <h3 class="uk-card-title">Transactions</h3>
        <ul class="uk-list">
          <li v-for="tx in transactions" :key="tx.hash">
            <router-link v-bind:to="{ name: 'transactions', params: { id: tx.hash }}">{{ tx.hash }}</router-link>
            Inputs
            <ul>
              <li v-for="i in tx.transaction.inputs" :key="i">
                {{ i }}
              </li>
            </ul>
            Outputs
            <ul>
              <li v-for="o in tx.transaction.outputs" :key="o">
                {{ o }}
              </li>
            </ul>
          </li>
        </ul>
      </div>
    </div>
  </div>
</template>

<script>
export default {
  props: ['blocks', 'transactions', 'connect'],

  created () {
    this.connect()
  },

  computed: {
    orderedBlocks: function () {
      return this.blocks.sort(function(a, b) {
        return b.header.raw.number - a.header.raw.number
      })
    }
  }
}
</script>
