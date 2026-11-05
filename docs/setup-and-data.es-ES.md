# Configuración y datos: instalación, modelos, copias de seguridad y cola offline

Esta guía cubre la instalación y las actualizaciones, el trabajo opcional en segundo plano basado en modelos, las ubicaciones de datos y su conservación segura, y la cola offline de la CLI.

Idiomas: [English](setup-and-data.md) · [繁體中文](setup-and-data.zh-Hant.md) · [简体中文](setup-and-data.zh-Hans.md) · [Español](setup-and-data.es-ES.md)<br>
[Volver al README en español](../README.es-ES.md)

Contenido: [Instalación](#installation) · [Modelos y privacidad](#models) · [Copia de seguridad y eliminación](#backup-and-removal) · [Cola offline](#offline-queue)

<a id="installation"></a>

## Instalación

### Descarga y abre Wenlan

[Descarga la aplicación](https://github.com/wenlan-app/wenlan/releases/latest) y ábrela tras instalarla:

- **macOS (Apple Silicon):** abre el `.dmg` y arrastra Wenlan a Aplicaciones. La aplicación está firmada y notarizada.
- **Windows x64:** ejecuta el `-setup.exe`. Todavía no está firmado. Si SmartScreen muestra un aviso, confirma que lo has descargado de la página oficial de Releases antes de elegir "Más información" → "Ejecutar de todas formas".
- **Linux:** todavía no tiene versión de escritorio; sigue la [guía de configuración](setup-with-ai.md#install-the-runtime) para usar Wenlan con tus herramientas de IA sin la aplicación.

Al iniciar por primera vez, Wenlan descarga un modelo para la búsqueda local. Mantén la conexión a internet hasta terminar la configuración. [Detalles de descarga y privacidad](PRIVACY.md#when-wenlan-reaches-the-network).

### Otras opciones de instalación y actualizaciones

La organización en segundo plano y la actualización automática de Páginas son opcionales y necesitan un [modelo configurado](#models); es distinto del modelo de búsqueda que se descarga al iniciar por primera vez.

**Instalar la aplicación de macOS desde el terminal**

El instalador descarga la aplicación, comprueba su SHA-256 y la mueve a Aplicaciones:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/scripts/install-macos-app.sh)"
```

**Sin la aplicación de escritorio**

En macOS Apple Silicon:

```bash
npx -y wenlan setup
```

`npx` requiere Node.js. Si no lo tienes, ejecuta `curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash` y después `wenlan setup --basic`.

Esto descarga la CLI precompilada, el daemon y el conector MCP, inicia el entorno local y lo verifica. No se requiere toolchain de Rust ni Cargo. Linux x64/ARM64 con glibc tiene una [ruta de configuración automática de shell](setup-with-ai.md#install-the-runtime); Windows x64 utiliza el archivo correspondiente de [Releases](https://github.com/wenlan-app/wenlan/releases/latest). macOS Intel actualmente [no tiene una instalación completa soportada del runtime](../crates/wenlan-cli/README.md#macos-intel).

**Qué se instala y cómo actualizar**

La aplicación incluye el daemon, la CLI y el conector MCP. Inicia el daemon al abrirse y ofrece conectar los clientes detectados mediante el plugin de Claude Code o Codex, o una entrada MCP para otros clientes compatibles. La instalación sin interfaz ejecuta el mismo daemon sin ventana; en ambos casos, tus herramientas de IA usan la misma base de conocimiento local.

Para actualizar la aplicación de macOS, arrastra la nueva sobre la antigua y ábrela. Cierra manualmente Wenlan 0.17.0 y anteriores antes de hacerlo.

Instrucciones manuales y específicas por herramienta: [Configuración asistida por IA](setup-with-ai.md) · [Plugin de Claude Code](../plugin/README.md) · [Plugin de Codex](../plugin-codex/README.md) · [CLI y MCP](../crates/wenlan-cli/README.md).

<a id="models"></a>

## Modelos y privacidad

Puedes usar tu IA conectada para escribir páginas sin instalar otro modelo de lenguaje local. La organización y las actualizaciones automáticas en segundo plano son opcionales y requieren configurar un modelo.

- **La búsqueda es local.** El modelo de búsqueda se descarga al iniciar por primera vez y se ejecuta en tu equipo, sin clave de API.
- **La IA conectada puede recibir tus conocimientos.** Un cliente de IA en la nube puede enviar el contenido recuperado a su proveedor. Si eliges un modelo en la nube para organizar material, también recibirá lo necesario para esa tarea. El almacenamiento local no convierte esas interacciones en locales.
- **Las estadísticas de uso están desactivadas por defecto.** Si las activas, Wenlan envía recuentos limitados de operaciones, versión y plataforma, no tus conocimientos ni un identificador de instalación. [Detalles](PRIVACY.md#telemetry).

Consulta la [información de red y privacidad](PRIVACY.md#when-wenlan-reaches-the-network) para las descargas, comprobaciones de actualizaciones, imágenes remotas y acceso remoto opcional.

### Opciones de modelos y detalles técnicos

- **Recuperación base local:** El [modelo de embedding BGE](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q) se ejecuta a través de FastEmbed en tu máquina para la búsqueda híbrida y no necesita clave de API.
- **Síntesis opcional en el dispositivo:** El enriquecimiento y la síntesis de Páginas pueden usar [`Qwen3.5 4B`](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) o [`Qwen3.5 9B`](https://huggingface.co/unsloth/Qwen3.5-9B-GGUF), según la elección del usuario, a través de [llama.cpp](https://github.com/ggml-org/llama.cpp). Wenlan no descarga ni activa un modelo de lenguaje hasta que elijas uno.
- **Otros proveedores:** Un endpoint local compatible con OpenAI como Ollama o LM Studio, o un proveedor en la nube configurado, pueden suministrar el enriquecimiento y la síntesis basados en modelos.
- **MLX en un Mac:** Para usar un modelo en el formato MLX de Apple, cárgalo en LM Studio u Ollama y elige ese proveedor en los ajustes de Wenlan. El enriquecimiento y la síntesis lo usarán. Por esta vía aún no se hace la comprobación de cada frase con su fuente, porque solo acepta los modelos locales propios de Wenlan, así que las páginas se muestran como no comprobadas.
- **Divulgación de nube:** Si el endpoint del modelo que seleccionas es remoto, Wenlan envía los prompts del sistema y del usuario de esa tarea a ese endpoint. La recuperación local y la síntesis en el dispositivo permanecen en tu máquina.

Referencia completa del flujo de trabajo: [plugin/skills](../plugin/skills/README.md). Roles técnicos del modelo: [fundamentos técnicos](technical-foundations.md#model-roles).

<a id="backup-and-removal"></a>

## Copia de seguridad y eliminación

Tus páginas y notas de sesión son archivos Markdown; las memorias y el grafo se guardan en una base de datos local. Puedes conservar tus conocimientos al desinstalar la app.

### Ubicaciones predeterminadas

- Páginas y notas de sesión: `~/.wenlan/`.
- Base de datos y datos del entorno de ejecución: `~/Library/Application Support/wenlan/` en macOS, `~/.local/share/wenlan/` en Linux o `%LOCALAPPDATA%\wenlan\` en Windows.
- Tus datos en archivos: `files-mirror/` en la carpeta de la base de datos. Cada cambio se escribe primero en el registro de escrituras (`files-mirror/log/`) y después en la base de datos; los archivos legibles de `files-mirror/files/` se ponen al día en pocos minutos. La base de datos es solo un índice de estos archivos: si al iniciar falta el archivo de la base de datos o está vacío, Wenlan la reconstruye a partir de los archivos y del registro, con los últimos cambios incluidos, y vuelve a comprobar las páginas con sus fuentes en segundo plano. La comprobación reutiliza los juicios anteriores de la IA, así que el texto que no ha cambiado no necesita otros nuevos. Una vez al día Wenlan comprueba además que los archivos reconstruyen la misma base de datos. No borres esta carpeta. `WENLAN_FILES_MIRROR=0` la desactiva; entonces solo se escribe la base de datos, como en versiones anteriores.

### Copia de seguridad de tus conocimientos

Cierra la app y detén el servicio en segundo plano antes de copiar estas carpetas. Incluye cualquier carpeta de páginas o datos que hayas configurado aparte. Exportar la wiki no equivale a una copia completa de la base de datos.

Si actualizaste desde Origin, comprueba también `~/.origin/` y la carpeta hermana de datos `origin` de tu plataforma. Inclúyelas al respaldar material antiguo o elimínalas solo si quieres borrarlo.

### Desinstalación

Desactiva *Ejecutar Wenlan en segundo plano al iniciar sesión* en Ajustes, cierra la app y elimina `Wenlan.app` o ejecuta el desinstalador de Windows. Las carpetas de conocimientos se conservan; bórralas solo si ya no necesitas los datos y tienes las copias de seguridad necesarias.

`wenlan background off` detiene el daemon y desactiva el arranque automático, pero no elimina el registro del servicio. Para desinstalar solo la CLI o revisar ajustes restantes de clientes de IA, credenciales y otros archivos, consulta los [detalles de eliminación](PRIVACY.md#data-deletion).

<a id="offline-queue"></a>

## Cola offline (outbox)

Si el daemon local no está accesible, `wenlan capture` y `wenlan brief update` escriben sus solicitudes en una cola local duradera (outbox) y terminan correctamente. Cuando el daemon vuelve, drena esas escrituras por las rutas HTTP normales; revisa la cola con `wenlan outbox status` o pide una reproducción inmediata con `wenlan outbox drain`. Una escritura que el daemon rechaza de plano (un 4xx, por ejemplo al no pasar el control de calidad del contenido) se mueve a `outbox/failed/` con un recibo en lugar de reintentarse para siempre; un fallo de transporte o un error del servidor (5xx) la deja en la cola para el siguiente drenaje, que se ejecuta automáticamente cada 60 segundos.
