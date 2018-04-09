<template>
  <div>
    <h3>Send Transaction</h3>
    <vue-json-editor v-model="json" :show-btns="false"></vue-json-editor>
    <button v-on:click="send" class="uk-button uk-button-default">Send</button>
  </div>
</template>

<script>
import vueJsonEditor from 'vue-json-editor'

export default {
  data () {
    return {
      json: {
        version: 1,
        inputs: [
          {
            previous_output: {
              hash: "0x0000000000000000000000000000000000000000000000000000000000000000",
              index: 0,
            },
            unlock: [0],
          },
        ],
        outputs: [
          {
            module: 0,
            capacity: 0,
            data: [0],
            lock: [0],
            recipient: {
              module: 0,
              lock: [0]
            }
          }
        ],
        groupings: [
          {
            transform_count: 0,
            destroy_count: 0,
            create_count: 0,
          }
        ]
      }
    }
  },

  components: {
    vueJsonEditor
  },

  methods: {
    send () {
      this.$emit("send_transaction", this.json)
    }
  }
}
</script>
