# Lazysodium JNA
-keep class com.sun.jna.** { *; }
-keep class com.goterl.lazysodium.** { *; }

# Gson
-keepattributes Signature
-keepattributes *Annotation*
-keep class com.agentdash.app.model.** { *; }

# OkHttp
-dontwarn okhttp3.internal.platform.**
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**
