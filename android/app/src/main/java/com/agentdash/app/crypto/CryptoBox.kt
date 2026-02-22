package com.agentdash.app.crypto

import com.goterl.lazysodium.LazySodiumAndroid
import com.goterl.lazysodium.SodiumAndroid
import com.goterl.lazysodium.interfaces.Box
import java.security.MessageDigest
import android.util.Base64

/**
 * NaCl crypto_box wrapper using X25519 + XSalsa20-Poly1305.
 * Compatible with Rust's crypto_box::SalsaBox used by the daemon relay connector.
 */
class CryptoBox private constructor(
    private val lazySodium: LazySodiumAndroid,
    private val mySecretKey: ByteArray,
    private val theirPublicKey: ByteArray
) {
    companion object {
        private val lazySodium = LazySodiumAndroid(SodiumAndroid())

        /**
         * Generate a new X25519 keypair.
         * Returns (secretKey, publicKey) as raw byte arrays.
         */
        fun generateKeypair(): Pair<ByteArray, ByteArray> {
            val publicKey = ByteArray(Box.PUBLICKEYBYTES)
            val secretKey = ByteArray(Box.SECRETKEYBYTES)
            lazySodium.cryptoBoxKeypair(publicKey, secretKey)
            return Pair(secretKey, publicKey)
        }

        /**
         * Derive channel_id from channel_secret: hex(SHA-256(secret)).
         */
        fun deriveChannelId(channelSecret: ByteArray): String {
            val digest = MessageDigest.getInstance("SHA-256")
            val hash = digest.digest(channelSecret)
            return hash.joinToString("") { "%02x".format(it) }
        }

        /**
         * Create a CryptoBox for encrypting/decrypting messages with a peer.
         */
        fun create(mySecretKey: ByteArray, theirPublicKey: ByteArray): CryptoBox {
            return CryptoBox(lazySodium, mySecretKey, theirPublicKey)
        }

        fun encodeBase64(data: ByteArray): String =
            Base64.encodeToString(data, Base64.NO_WRAP)

        fun decodeBase64(data: String): ByteArray =
            Base64.decode(data, Base64.DEFAULT)
    }

    /**
     * Encrypt a plaintext message.
     * Returns (ciphertext, nonce) as raw byte arrays.
     */
    fun encrypt(plaintext: ByteArray): Pair<ByteArray, ByteArray> {
        val nonce = lazySodium.randomBytesBuf(Box.NONCEBYTES)
        val ciphertext = ByteArray(plaintext.size + Box.MACBYTES)

        val success = lazySodium.cryptoBoxEasy(
            ciphertext,
            plaintext,
            plaintext.size.toLong(),
            nonce,
            theirPublicKey,
            mySecretKey
        )

        if (!success) {
            throw CryptoException("Encryption failed")
        }

        return Pair(ciphertext, nonce)
    }

    /**
     * Encrypt a string message. Returns (ciphertextBase64, nonceBase64).
     */
    fun encryptString(plaintext: String): Pair<String, String> {
        val (ciphertext, nonce) = encrypt(plaintext.toByteArray(Charsets.UTF_8))
        return Pair(encodeBase64(ciphertext), encodeBase64(nonce))
    }

    /**
     * Decrypt a ciphertext with the given nonce.
     * Returns the plaintext as a byte array.
     */
    fun decrypt(ciphertext: ByteArray, nonce: ByteArray): ByteArray {
        val plaintext = ByteArray(ciphertext.size - Box.MACBYTES)

        val success = lazySodium.cryptoBoxOpenEasy(
            plaintext,
            ciphertext,
            ciphertext.size.toLong(),
            nonce,
            theirPublicKey,
            mySecretKey
        )

        if (!success) {
            throw CryptoException("Decryption failed")
        }

        return plaintext
    }

    /**
     * Decrypt base64-encoded ciphertext and nonce. Returns plaintext string.
     */
    fun decryptString(ciphertextB64: String, nonceB64: String): String {
        val ciphertext = decodeBase64(ciphertextB64)
        val nonce = decodeBase64(nonceB64)
        val plaintext = decrypt(ciphertext, nonce)
        return String(plaintext, Charsets.UTF_8)
    }
}

class CryptoException(message: String) : Exception(message)
