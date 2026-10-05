plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "app.mush"
    compileSdk = 34

    defaultConfig {
        applicationId = "app.mush"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.6")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.6")
    implementation(platform("androidx.compose:compose-bom:2024.09.03"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.navigation:navigation-compose:2.8.0")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    // Этап B день 6 (MUSH-B-SPEC §2): идентикон — Coil + SvgDecoder, НЕ
    // WebView, НЕ Compose Canvas (Rust — единственный источник правды рендеринга).
    implementation("io.coil-kt:coil-compose:2.6.0")
    implementation("io.coil-kt:coil-svg:2.6.0")
    // Этап B (MUSH-B-SPEC §1.1): SQLCipher bundled AAR. Версия фиксируется
    // нами (не ОС); ABI-набор AAR (arm64-v8a/armeabi-v7a/x86_64) совпадает
    // с libmush_jni.so. 4.17.0 — последняя с minCompileSdk ≤ 34: 4.18.0+
    // требует compileSdk 37 (проект на 34, AGP 8.6.1).
    implementation("net.zetetic:sqlcipher-android:4.17.0")
    // SQLCipher 4.17+ ссылается на androidx.sqlite-типы в сигнатурах API.
    implementation("androidx.sqlite:sqlite:2.4.0")
    testImplementation("junit:junit:4.13.2")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
