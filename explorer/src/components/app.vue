<template>
  <div class="uk-container">
    <ul class="uk-tab uk-child-width-expand">
      <li><router-link to="/">Home</router-link></li>
      <li><router-link to="/send_transaction">Send Transaction</router-link></li>
      <li><router-link to="/blocks">Blocks</router-link></li>
      <li><router-link to="/transactions">Transactions</router-link></li>
      <li>
        <div class="uk-inline">
          <span class="uk-form-icon" uk-icon="icon: server"></span>
          <input v-model="addr" v-on:keyup.enter="connect" type="text" class="uk-input"/>
        </div>
      </li>
    </ul>

    <router-view 
      :blocks="blocks" 
      :transactions="transactions"
      :block="block"
      :transaction="transaction"
      :get_block="get_block"
      :get_transaction="get_transaction"
      :connect="connect"
      v-on:send_transaction="send_transaction">
    </router-view>
  </div>
</template>

<script>
import _ from "lodash"
import JsonRPC from "simple-jsonrpc-js"

export default {
  data: function() {
    return {
      addr: 'http://localhost:3030/',
      blocks: [],
      transactions: [],
      block: {},
      transaction: {},
    }
  },
  methods: {
    connect: function() {
      this.jrpc().call('get_tip_header', []).then((result) => {
        this.blocks = []
        this.transactions = []
        _.range(result.raw.number, result.raw.number - 10, -1).forEach((height, _) => {
          this.jrpc().call('get_block_hash', [height]).then((hash) => {
            this.jrpc().call('get_block', [hash]).then((block) => {
              this.blocks.push(block)
              this.transactions = this.transactions.concat(block.transactions)
            })
          })
        })
      })
    },

    get_block: function(hash) {
      this.jrpc().call('get_block', [hash]).then((block) => {
        this.block = block
        console.log(block)
      })
    },

    get_transaction: function(hash) {
      this.jrpc().call('get_transaction', [hash]).then((transaction) => {
        this.transaction = transaction
        console.log(transaction)
      })
    },

    send_transaction(tx) {
      this.jrpc().call('send_transaction', [tx]).then((result) => {
        console.log(result)
      })
    },

    jrpc: function() {
      let jrpc = new JsonRPC()
      let addr = this.addr

      jrpc.toStream = function(msg) {
          let xhr = new XMLHttpRequest()
          xhr.onreadystatechange = function() {
              if (this.readyState != 4) return

              try {
                  JSON.parse(this.responseText)
                  jrpc.messageHandler(this.responseText)
              } catch (e) {
                  console.error(e)
              }
          }

          xhr.open("POST", addr, true)
          xhr.setRequestHeader('Content-type', 'application/json')
          xhr.send(msg)
      }
      return jrpc
    }
  }
}
</script>
