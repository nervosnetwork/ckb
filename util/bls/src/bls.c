#include <stdint.h>
#include <string.h>
#include <pbc.h>

 char *param = "type f\n\
 q 205523667896953300194896352429254920972540065223\n\
 r 205523667896953300194895899082072403858390252929\n\
 b 40218105156867728698573668525883168222119515413\n\
 beta 115334401956802802075595682801335644058796914268\n\
 alpha0 191079354656274778837764015557338301375963168470\n\
 alpha1 71445317903696340296199556072836940741717506375";

 void key_gen_c(uint8_t* out_sk, size_t *sk_len, uint8_t* out_pk, size_t *pk_len, uint8_t* out_g, size_t *g_len) {
    pairing_t pairing;
    element_t g;
    element_t public_key, secret_key;

    pairing_init_set_buf(pairing, param, strlen(param));
    element_init_G2(g, pairing);
    element_init_G2(public_key, pairing);
    element_init_Zr(secret_key, pairing);
    element_random(g);
    element_random(secret_key);
    element_pow_zn(public_key, g, secret_key);

    *sk_len = element_length_in_bytes(secret_key);
    element_to_bytes(out_sk, secret_key);

    *pk_len = element_length_in_bytes_compressed(public_key);
    element_to_bytes_compressed(out_pk, public_key);

    *g_len = element_length_in_bytes_compressed(g);
    element_to_bytes_compressed(out_g, g);

    element_clear(secret_key);
    element_clear(public_key);
    element_clear(g);

 }
 
 void sign_c(uint8_t* out, size_t *out_len, uint8_t* msg, size_t msg_len, uint8_t* data) {
    pairing_t pairing;
    element_t secret_key;
    element_t sig;
    element_t h;
    
    pairing_init_set_buf(pairing, param, strlen(param));
    
    element_init_G1(h, pairing);
    element_init_G1(sig, pairing);
    element_init_Zr(secret_key, pairing);

    element_from_bytes(secret_key, data);
    element_from_hash(h, msg, msg_len);
    element_pow_zn(sig, h, secret_key);
    *out_len = element_length_in_bytes_compressed(sig);
    element_to_bytes_compressed(out, sig);
    
    element_clear(sig);
    element_clear(secret_key);
    element_clear(h);
 }

 int verify_c(uint8_t* msg, size_t msg_len, uint8_t* data_s, uint8_t* data_g, uint8_t* data_p) {
    pairing_t pairing;
    element_t public_key;
    element_t sig;
    element_t g, h;
    element_t temp1, temp2;

    pairing_init_set_buf(pairing, param, strlen(param));

    element_init_G2(g, pairing);
    element_init_G2(public_key, pairing);
    element_init_G1(sig, pairing);
    element_init_G1(h, pairing);
    element_init_GT(temp1, pairing);
    element_init_GT(temp2, pairing);

    element_from_bytes_compressed(public_key, data_p);
    element_from_bytes_compressed(g, data_g);
    element_from_bytes_compressed(sig, data_s);

    element_from_hash(h, msg, msg_len);

    pairing_apply(temp1, sig, g, pairing);
    pairing_apply(temp2, h, public_key, pairing);

    int r = !element_cmp(temp1, temp2);

    element_clear(sig);
    element_clear(public_key);
    element_clear(g);
    element_clear(h);
    element_clear(temp1);
    element_clear(temp2);
    pairing_clear(pairing);

    return r;
}
