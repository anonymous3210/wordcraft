# WordCraft para Android ARM64 (experimental)

Este é o alvo Android nativo do WordCraft. Ele reutiliza o motor e a interface egui do workspace completo, sem WebView. O APK é compilado para `arm64-v8a` (`aarch64-linux-android`) e requer Android 8.0/API 26 ou posterior.

## Compilar do zero no Linux ou WSL 2

### 1. Instale as ferramentas

Instale Rust pelo [rustup](https://rustup.rs/), o **JDK 21 completo** (precisa incluir `java`, `javac` e `keytool`) e as Android Command-line Tools. Com `ANDROID_HOME` apontando para o SDK, aceite a licença Android interativamente e instale as versões usadas por este alvo:

```sh
export ANDROID_HOME="$HOME/Android/Sdk"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --licenses
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --install \
  "platform-tools" \
  "platforms;android-35" \
  "build-tools;35.0.0" \
  "ndk;30.0.16248370"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/30.0.16248370"
```

O projeto fixa o API de compilação em 35, o Build Tools em 35.0.0 e o NDK em 30.0.16248370. As licenças precisam ser aceitas no computador antes do primeiro build.

### 2. Clone a branch Android

```sh
git clone --branch android-native-arm64 https://github.com/anonymous3210/wordcraft.git
cd wordcraft
```

Se o repositório já estiver clonado, entre na pasta e faça checkout da branch `android-native-arm64`.

### 3. Gere o APK

```sh
./apps/wordcraft-android/build.sh
```

O script verifica as ferramentas, instala `cargo-apk2` 1.5.0 se estiver faltando, adiciona o target Rust ARM64 e compila usando o `Cargo.lock` do repositório. O arquivo de saída é:

```text
target/debug/apk/wordcraft.apk
```

Para instalar num aparelho ARM64 conectado com depuração USB:

```sh
adb install -r target/debug/apk/wordcraft.apk
```

Também é possível executar manualmente, sem o script, depois de preparar as ferramentas:

```sh
rustup target add aarch64-linux-android
cargo install cargo-apk2 --version 1.5.0 --locked
cargo apk2 build --manifest-path apps/wordcraft-android/Cargo.toml
```

## Build release

Uma build release otimizada exige uma chave de assinatura própria. Guarde o arquivo e a senha fora do repositório e não reutilize a chave de depuração:

```sh
export CARGO_APK_RELEASE_KEYSTORE="$HOME/.android/wordcraft-release.jks"
read -rsp "Senha do keystore: " CARGO_APK_RELEASE_KEYSTORE_PASSWORD; echo
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD
cargo apk2 build --release --manifest-path apps/wordcraft-android/Cargo.toml
unset CARGO_APK_RELEASE_KEYSTORE CARGO_APK_RELEASE_KEYSTORE_PASSWORD
```

O APK otimizado sai em `target/release/apk/wordcraft.apk`. O APK gerado pelo script padrão é de depuração e serve para instalação/teste; não o publique numa loja.

## Arquivos no Android 11 e 12 (SAF)

O app usa o seletor do sistema por meio do Storage Access Framework: `ACTION_OPEN_DOCUMENT` para abrir e `ACTION_CREATE_DOCUMENT` para salvar. A seleção entrega acesso somente aos documentos escolhidos; o app copia a leitura para seu diretório privado e grava na URI do provedor apenas quando o usuário executa Save/Save As ou Export. O identificador SAF é persistido junto com a permissão URI persistível; se o provedor recusar essa permissão, a seleção falha em vez de guardar uma URI que deixaria de funcionar após reiniciar o app. Não são solicitadas permissões amplas como `MANAGE_EXTERNAL_STORAGE`, `READ_EXTERNAL_STORAGE` ou `WRITE_EXTERNAL_STORAGE`.

No Android, **AutoSave é um cache privado de recuperação**, não uma gravação automática no arquivo escolhido. Com AutoSave ligado, alterações não salvas são guardadas periodicamente em `wordcraft-autosave-cache` dentro do armazenamento privado do app; depois de uma falha, use **File › Recover** para restaurar a cópia. O botão Save continua sendo necessário para gravar no documento SAF. Documentos protegidos por senha não são colocados no cache, pois as cópias de recuperação não são criptografadas. Desligar AutoSave remove a cópia de recuperação mantida para esse documento.

O fluxo foi direcionado a Android 11/API 30 e Android 12/API 31. Como determina o SAF, o seletor não concede acesso às pastas `Android/data` ou `Android/obb`; esse app não tenta contornar essa restrição. Um erro do provedor durante a gravação pode deixar o arquivo remoto parcial; a cópia de trabalho privada é mantida, mas a atomicidade depende do provedor. [Documentação oficial do SAF](https://developer.android.com/training/data-storage/shared/documents-files) · [Mudanças de armazenamento no Android 11](https://developer.android.com/about/versions/11/privacy/storage).

A subclasse Android é empacotada pelo [`cargo-apk2`](https://crates.io/crates/cargo-apk2); o bridge de arrays usa a API `JObjectArray::len`/`get_element` do [JNI 0.22.4](https://docs.rs/jni/0.22.4/jni/objects/struct.JObjectArray.html).

## Escopo atual

A inicialização, o renderizador, a interface e a seleção de documentos são nativos. O SAF cobre abrir, salvar, Salvar como, exportar PDF/PNG, salvar as páginas escolhidas como PDF e importar arquivos escolhidos; AutoSave usa somente o cache privado descrito acima. Impressão em impressora física e compartilhamento ainda não estão conectados. A interface existente foi desenhada para ribbon de desktop; teclado virtual, composição/IME e ergonomia precisam ser validados num aparelho.
