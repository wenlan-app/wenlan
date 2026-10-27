<!-- README_SYNC: source=README.md sha256=51a95b2103002be792aa2bb8b7946373a7bcd5bf31489bacca847da7bf0115f1 -->

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-es-ES-mobile.png">
    <img src="./docs/assets/readme-banner-es-ES.png" alt="Wenlan: Tu wiki viva. Gratis, de código abierto y local-first." width="100%">
  </picture>
</p>

<p align="center">
  <strong>Notas, código y chats de IA en una wiki, contrastada con sus fuentes.</strong><br>
  Una sola fuente de verdad para ti y tus herramientas de IA, al día cuando cambian tus fuentes.<br>
  Tus notas se quedan en tu equipo y solo van a las herramientas de IA que elijas.
</p>

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-Hans.md">简体中文</a> | <a href="./README.zh-Hant.md">繁體中文</a> | Español
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><strong>Descargar</strong></a> ·
  <a href="#start-in-30-seconds">Primeros&nbsp;pasos</a> ·
  <a href="#what-can-i-bring-in">Fuentes</a> ·
  <a href="#learn-more">Docs</a> ·
  <a href="https://discord.gg/nmzVTTU3HG">Discord</a>
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml?query=branch%3Amain"><img alt="CI" src="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml/badge.svg?branch=main&event=push"></a>
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><img alt="Última versión" src="https://img.shields.io/github/v/release/wenlan-app/wenlan?label=release"></a>
  <a href="#license"><img alt="Licencia: Apache-2.0 y AGPL-3.0" src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20AGPL--3.0-blue.svg"></a>
</p>

<!-- R113 walkthrough: approved 0.5-second cover; 1080p; original video/audio and masters retained. -->

<p align="center">
  <a href="https://github.com/user-attachments/assets/5e661389-d7ea-4a36-a645-2b1ec2a92d32"><img src="./docs/assets/readme-video-cover-en.png" alt="Ver el recorrido por Wenlan" width="100%"></a>
</p>

<a id="why-wenlan"></a>

<br>

## Por qué Wenlan

- **Tu IA la escribe y la mantiene al día:** la herramienta de IA que ya usas convierte tus notas, archivos y chats de IA en páginas, sin otro modelo.
- **Te dice lo que no puede respaldar:** se rechaza una página cuando la mayoría de sus citas no coinciden con la fuente, y cada cita abre el original.
- **Marca lo que cambió:** cuando cambia una fuente, una página o una línea de código, solo se marcan las páginas que dependen de ella.
- **Lo que escribes sigue siendo tuyo:** Wenlan nunca edita tus notas existentes sin tu permiso, y las páginas que escribiste quedan en **Ask me first**.
- **Trae lo que ya tienes y consérvalo al cambiar de herramienta:** notas, documentos, enlaces y chats entran ([lista completa](#what-can-i-bring-in)) como Markdown local que sirve en las herramientas de IA que elijas.

<a id="quickstart"></a>
<a id="start-in-30-seconds"></a>
<a id="start-with-the-app"></a>
<a id="open-the-wiki"></a>
<a id="desktop-app"></a>
<a id="claude-code-in-30-seconds"></a>
<a id="codex-plugin"></a>
<a id="mcp-setup"></a>
<a id="mcp-clients"></a>

<br>

## Primeros pasos

### 1. Descarga y abre Wenlan

[Descarga la aplicación de escritorio](https://github.com/wenlan-app/wenlan/releases/latest) para macOS 11 o posterior en Apple Silicon, o Windows x64. La app está en inglés, chino simplificado y chino tradicional.

¿Prefieres el terminal? En un Mac, ejecuta `npx -y wenlan setup`, o `brew install 7xuanlu/tap/wenlan` y después `wenlan setup`. Linux y Windows: [guía de instalación de la CLI](docs/setup-and-data.es-ES.md#installation).

### 2. Trae tu material

Crea una página o trae una bóveda de Obsidian, un grafo de Logseq, una carpeta o chats de IA anteriores. [Guía de importación](docs/import-guide.md).

### 3. Conecta tu herramienta de IA

| Herramienta de IA | Conexión | Configuración |
|---|---|---|
| Claude Code / Claude Desktop | Plugin / MCP | [Guía de Claude](docs/setup-with-ai.md#claude-code) |
| ChatGPT Desktop (Codex), Codex CLI | Plugin | [Guía de Codex](docs/setup-with-ai.md#codex) |
| Cursor, GitHub Copilot, VS Code, Gemini CLI, Antigravity, Windsurf, Kiro, OpenCode, Kimi Code | MCP y skills | [Guía de MCP](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |
| Muse Code y otros clientes MCP | MCP | [Guía de MCP](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |

En los ajustes de la aplicación, **Add a tool** encuentra las herramientas de IA de tu ordenador y **Connect all** las configura con un solo clic. Después, inicia una nueva sesión de IA y pide:

> /recall mis páginas y decisiones anteriores sobre esto y luego ayúdame con: …

En Codex, usa `$recall`; con una herramienta solo MCP, pídele que consulte Wenlan primero.

Cuando termina una importación, la aplicación ofrece **Write pages with** tu herramienta de IA para convertirla en tus primeras páginas con citas. No hace falta ningún comando.

<a id="what-can-i-bring-in"></a>

<br>

## Fuentes e importaciones

| Qué puedes importar | Para qué sirve |
| --- | --- |
| Markdown, texto, PDF con texto, DOCX o una carpeta | Busca y cita el texto importado. Importa una carpeta una vez o mantenla sincronizada cuando cambien sus archivos. |
| Una bóveda de Obsidian o un grafo de Logseq | Busca y cita notas existentes. Mantenla sincronizada para seguir los cambios en segundo plano, o impórtala una vez. Wenlan nunca edita tus notas existentes sin tu permiso: en una bóveda de Obsidian, las notas en **Ask me first** reciben sugerencias de la IA que aceptas o rechazas. |
| Enlaces web | Pega un enlace en una página y se guarda como fuente. Los enlaces de YouTube, X, Threads, Instagram y RedNote se muestran como tarjeta, vídeo reproducible o la publicación original. También puedes pegar un texto con su URL para guardar ese extracto. |
| Gmail, Notion, Google Drive, Slack, GitHub | Tu herramienta de IA los lee con sus propios conectores y guarda en Wenlan lo importante con un enlace al original. |
| Un repositorio de código | `/wenlan wiki` escribe páginas vinculadas a las líneas de código en las que se basan. [Wiki de código](#code-repository-wiki). |
| Un paquete OKF | Conecta como fuente una carpeta Open Knowledge Format, como una OpenWiki o una exportación de Wenlan. |
| Tus chats de IA mientras trabajas | Claude Code, Codex y tus otras herramientas de IA guardan como memorias las decisiones, hallazgos y avances de cada conversación, y las páginas las citan. No se importan las transcripciones completas. |
| ZIP de exportación de ChatGPT o Claude | Recupera conversaciones anteriores, incluido un último mensaje que no obtuvo respuesta, y cítalas como contexto; la importación es una instantánea, no una conexión activa a la cuenta. |
| Memorias existentes como texto | Guarda preferencias, decisiones o aprendizajes como memorias individuales. |

Los PDF escaneados necesitan extracción de texto; los adjuntos de chats no se importan. [Guía de importación (en inglés)](docs/import-guide.md).

<a id="what-makes-wenlan-distinct"></a>
<a id="why-is-wenlan-different"></a>
<a id="two-lifecycles"></a>
<a id="two-lifecycles-one-maintained-knowledge-system"></a>
<a id="dos-ciclos-de-vida-un-sistema-de-conocimiento-mantenido"></a>
<a id="local-markdown"></a>
<a id="memoria-atómica"></a>
<a id="página-mantenida"></a>
<a id="markdown-local-que-funciona-con-obsidian"></a>

<a id="mantén-el-conocimiento-al-día-y-conserva-su-historial"></a>

<br>

## Steep: mantén útil el conocimiento cuando cambia

<p align="center">
  <img src="./docs/assets/wenlan-updates-history.png" alt="Flujo ilustrativo: un hallazgo en un documento actualiza la memoria de V1 a V2 y después influye en una nota, que se actualiza automáticamente o queda pendiente de revisión." width="100%">
</p>
<p>Ilustración basada en el comportamiento actual de actualización; no es una captura de la app.</p>

Guardar material es solo el comienzo. **Steep lo prepara para reutilizarlo y mantiene al día las páginas compatibles**, para que tú y tu IA podáis continuar a partir de hallazgos anteriores sin volver a procesar los originales.

Elige uno de dos modos en Settings:

- **On demand** (las instalaciones nuevas empiezan aquí): cuando tu herramienta de IA guarda o escribe en Wenlan, hace Steep sobre la marcha y, mientras trabaja, sabe qué páginas están desactualizadas y qué material aún no está en ninguna página. No hace falta ningún comando; `/wenlan steep` está ahí si quieres ponerlo todo al día de una vez.
- **Automatic:** Wenlan hace Steep en segundo plano con el modelo que asignes.

Qué hace Steep:

- **Enriquece las memorias:** Añade tipos, detalles estructurados, pistas de búsqueda y enlaces al grafo para facilitar la recuperación contextual de cada hallazgo.
- **Sigue la evidencia:** Los cambios en fuentes, memorias o código citado marcan las páginas dependientes para actualizarlas; las memorias nuevas también pueden enriquecer páginas compatibles. Si un borrador no supera las comprobaciones de citas, se conserva la página guardada. Superarlas no demuestra que los datos sean correctos.
- **Conserva el control y el historial:** La actualización en segundo plano modifica las páginas compatibles mantenidas por IA; las páginas que hayas escrito o editado reciben propuestas de revisión. Las sustituciones explícitas ocultan o archivan las memorias anteriores y conservan sus enlaces. Las revisiones de páginas se pueden inspeccionar.
- **Ask me first:** Con **Ask me first on pages I wrote** activado, como en las instalaciones nuevas, los cambios de la IA en páginas que escribiste o editaste esperan tu visto bueno. El ajuste **AI changes** de cada página muestra **Allowed** o **Ask me first**. Las páginas que escribió la IA siguen actualizándose.

<a id="what-it-does-on-its-own"></a>

### Lo que hace por su cuenta

En ambos modos, sin modelo, Wenlan:

- **Marca páginas desactualizadas** cuando cambian sus fuentes, memorias o el código citado. Las páginas de la wiki de un repositorio muestran qué cambió en el código.
- **Sugiere fusionar páginas casi duplicadas** con una tarjeta que tú o tu IA aceptan o descartan.
- **Ordena los enlaces:** lista los `[[enlaces]]` que aún no llevan a ninguna página y quita las citas a memorias que borraste.

Reescribir una página marcada requiere un modelo. En **Automatic**, Wenlan lo hace en segundo plano. En **On demand**, tu herramienta de IA sabe qué páginas esperan y puede ponerlas al día mientras trabaja.

[Guía de Steep: activadores, reglas de revisión y modelos (en inglés)](docs/steep.md). Elige un modelo en el dispositivo, un servidor local o un proveedor en la nube; el trabajo en segundo plano con modelos es opcional.

<a id="code-repository-wiki"></a>

<br>

## Una wiki para tu repositorio de código

Ejecuta `/wenlan wiki` (`$wenlan wiki` en Codex) en un repositorio. Tu herramienta de IA planifica la wiki a partir del propio repositorio: una página de resumen que enumera todas las páginas y, después, una página para cada parte o flujo que necesita conocer alguien nuevo. Una herramienta pequeña tiene pocas páginas; un monorepo grande, muchas más. Al volver a ejecutarlo solo escribe las páginas que faltan.

- **Ligada al código:** Cada página registra las líneas de código de las que se extrajo. Cuando ese código cambia, la página muestra qué cambió con un solo botón Review, y Steep pone la página al día.
- **En el repositorio como pull request:** El botón de GitHub de un Espacio lo conecta con su repositorio y coloca las páginas que dependen de ese código en la carpeta `wiki/` del repositorio como pull request. Ejecútalo desde el botón o activa la actualización diaria: usa tu propia herramienta de IA en tu ordenador, o se ejecuta en GitHub Actions con la clave de tu herramienta de IA guardada como secret del repositorio, así tu ordenador puede estar apagado. La misma pull request corrige las frases de la documentación del propio repositorio con las que el código ya no coincide, para que las revises.
- **Tus ediciones se conservan:** Una página de la wiki que alguien editó en el repositorio conserva esa edición. Las páginas que citan tus memorias o chats no entran en el repositorio.

<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>
<a id="qué-es-esto"></a>
<a id="what-is-this"></a>
<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="knowledge-graph"></a>

<a id="explora-y-organiza-tu-conocimiento"></a>

<a id="conecta-tu-conocimiento"></a>

<br>

## Organiza la información dispersa en conocimiento

<a id="explora-tu-grafo-de-conocimiento-personal"></a>

<p align="center">
  <img src="./docs/assets/wenlan-personal-graph.png" alt="Grafo real de Wenlan con regiones ilustrativas de intereses personales, investigación y desarrollo" width="100%">
</p>
<p>Nodos y conexiones de la app; los nombres y límites de las regiones son ilustrativos.</p>

Las **fuentes** conservan documentos y conversaciones; las **memorias** guardan decisiones, aprendizajes y motivos que conviene reutilizar. Combínalas en **páginas Markdown editables con citas**, enlázalas con `[[wikilinks]]` y sigue las citas hasta los originales. Las páginas pueden incluir diagramas Mermaid, que la aplicación dibuja; cuando una herramienta de IA escribe una página, Wenlan revisa sus diagramas en busca de errores comunes e indica la línea que hay que corregir.

- **Memorias atómicas:** Siguiendo el [principio de atomicidad de Zettelkasten](https://zettelkasten.de/posts/create-zettel-from-reading-notes/), cada hallazgo puede recuperarse y reutilizarse por separado, sin cargar una conversación o página completa.
- **Temas compartidos:** Con un modelo de enriquecimiento configurado, la extracción y resolución de entidades mediante LLM vinculan menciones de la misma persona, proyecto o concepto en documentos y conversaciones, conservando sus memorias y fuentes.
- **Comunidades de conocimiento:** Agrupa temas conectados para descubrir relaciones en tu material. Label Propagation se ejecuta de forma predeterminada; Leiden está disponible como algoritmo opcional. [Detalles del grafo](docs/technical-foundations.md#graph-data-and-entity-resolution).

<a id="daily-workflow"></a>

<br>

## Uso diario

Guarda los nuevos hallazgos y avances mientras trabajas y empieza la próxima sesión con ellos.

<p align="center">
  <img src="./docs/assets/wenlan-daily-workflow-es-ES.png" alt="Las fuentes y memorias previas se acumulan en Wenlan; úsalas en la aplicación o con IA y guarda nuevas memorias y el progreso para continuar después." width="100%">
</p>

Flujo simplificado. Los comandos del plugin y la organización de páginas son opcionales, sin un orden fijo.

- **Trabaja en la aplicación.** Añade material, escribe o edita páginas, consulta las citas y revisa los cambios propuestos sin conectar una IA. El icono de la barra de menús (o de la bandeja) abre un panel pequeño con lo que necesita tu atención, tus páginas recientes y Quick capture.
- **Usa tu conocimiento y escribe páginas nuevas.** Pide a tu IA que recupere fuentes y memorias relevantes para la tarea (`/recall`). También puede escribir una página con citas o actualizar una existente a partir de su versión actual y guardarla en Wenlan. Cuando una página cita sus fuentes como `[n]`, Wenlan compara cada frase citada con su fuente y rechaza la página cuando la mayoría de sus citas no coinciden.
- **Conserva lo que aprendes.** Guarda una decisión o lección, junto con sus motivos y fuentes, en la aplicación o a través de tu IA (`/capture`).
- **Pausa y retoma.** Guarda hallazgos y un registro de la sesión, actualiza las páginas afectadas y anota el progreso y los siguientes pasos en el Brief del proyecto (`/handoff`). La próxima vez, recupera ese contexto para continuar (`/brief`).

**Organiza las memorias acumuladas cuando lo necesites:** Sintetiza o actualiza páginas con citas a partir de memorias relacionadas (`/wenlan distill`); no es necesario para escribir una página nueva. [Comandos del plugin](plugin/skills/README.md).

<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>

<a id="what-wenlan-is-not"></a>
<a id="retrieval"></a>
<a id="recuperación-a-través-de-palabras-significado-y-conexiones"></a>
<a id="retrieval-across-words-meaning-and-connections"></a>

<a id="usa-tu-conocimiento-con-ia"></a>

<br>

## Pon tu conocimiento a trabajar con IA

Un agente puede leer archivos y escribir texto. Wenlan añade una **base de conocimiento compartida y mantenida entre tareas y herramientas de IA**: recupera decisiones anteriores con sus fuentes, guarda el trabajo nuevo y sigue usándolo a medida que cambia la evidencia.

<a id="if-you-already-use-claude-code-on-your-notes"></a>

### Si ya usas Claude Code o Codex con tus notas

Sigue haciéndolo. Wenlan se conecta a la misma herramienta de IA y añade las comprobaciones que una carpeta de Markdown no hace por sí sola: rechaza una página cuando la mayoría de sus citas no coinciden con la fuente, marca las páginas cuando cambian sus fuentes, deja para tu revisión las ediciones de IA en páginas que escribiste y nunca edita tus notas existentes sin tu permiso.

**Úsala para:**

- **Desarrollo:** Incorpora decisiones técnicas, aprendizajes y criterios de aceptación anteriores al implementar y depurar.
- **Productos y sitios web:** Usa investigación de clientes, posicionamiento y principios de diseño para redactar especificaciones o crear páginas.
- **Investigación y aprendizaje:** Convierte lecturas y hallazgos guardados en informes citados o guías de estudio reutilizables.

**Recuperación y conexiones:** Los plugins orientan la recuperación y el guardado; las conexiones MCP exponen las herramientas para que las use tu IA.

- **[Búsqueda híbrida local](docs/technical-foundations.md#retrieval-pipeline):** Encuentra términos exactos y significados similares con SQLite FTS5 y embeddings BGE en un índice vectorial DiskANN de libSQL, combinados mediante Reciprocal Rank Fusion (RRF) ponderado; la búsqueda base no requiere clave API.
- **Clasificación asistida por el grafo:** Los enlaces entre entidades compartidas elevan memorias relacionadas aunque usen palabras distintas. El reranking opcional con cross-encoder compara cada candidato con la pregunta para ajustar el orden.
- **Recuperación limitada a un Espacio:** Selecciona un Espacio para limitar el conocimiento devuelto al proyecto o colección personal pertinente.

Activa el interruptor **Web access** en Settings para que clientes web de IA compatibles, como Claude, ChatGPT o Muse, puedan buscar en toda la biblioteca o en un Espacio elegido mientras el ordenador con Wenlan esté conectado. Al permitir una app eliges solo búsqueda o búsqueda y escritura; una app con escritura también puede añadir, editar y borrar, y las apps de IA pueden pedirte confirmación antes de cada cambio. Esta función es experimental; las consultas y los resultados pasan por un relay. Consulta los [límites de privacidad](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector).

<a id="architecture"></a>

<br>

## Arquitectura

**Una base local compartida.** La app, la CLI y las herramientas de IA conectadas leen y escriben mediante el servicio local de Wenlan. Las páginas son archivos Markdown; memorias, relaciones e índices de búsqueda residen en libSQL local.

**Hecho con** Rust para el servicio local, la CLI y el servidor MCP, y Tauri con React y TypeScript para la app de escritorio.

<p align="center">
  <img src="./docs/assets/wenlan-architecture-es-ES.png" alt="Vista de la arquitectura: la app y las herramientas de IA comparten una base de conocimiento local, con modelos opcionales para la organización en segundo plano." width="100%">
</p>

Esquema de la arquitectura. [Búsqueda, evidencias y modelos](docs/technical-foundations.md).

<a id="models-and-privacy"></a>

<br>

## Datos y privacidad

**Exporta tu wiki:** Exporta las páginas compatibles como una wiki Open Knowledge Format (OKF).

Un cliente de IA en la nube puede enviarle el conocimiento recuperado a su proveedor; un modelo en la nube también recibe el contenido que procesa. El almacenamiento local no mantiene locales esas interacciones. Las estadísticas de uso están desactivadas por defecto.

[Ubicación de archivos, copias y eliminación](docs/setup-and-data.es-ES.md#backup-and-removal) · [Red y privacidad](docs/PRIVACY.md#when-wenlan-reaches-the-network).

<a id="evaluation"></a>

<br>

## Evaluación

Estas mediciones de LongMemEval (LME) evalúan la recuperación de conocimiento guardado, no la calidad de las respuestas de IA. Recall@5 mide cuánto material relevante aparece entre los cinco primeros resultados.

<!-- EVAL_SNAPSHOT_START -->
| Benchmark | Recall@5 | MRR | NDCG@10 |
|---|---:|---:|---:|
| LME_Oracle (500 Q) | 93.6% | 0.857 | 0.883 |
| LME_S (deep, 90 Q) | 87.7% | 0.815 | 0.822 |
<!-- EVAL_SNAPSHOT_END -->

En una prueba con 10.000 páginas (páginas generadas, M2 Pro con 16 GB, v0.20.0), la búsqueda en la App respondió en 11 a 50 ms, el reinicio tardó menos de 0,3 s y la memoria llegó a un máximo de unos 1,2 GB. Con la corrección de búsqueda de 0.20.1, la búsqueda de las herramientas de IA encontró las 30 palabras raras de prueba en esa biblioteca.

Al importar una bóveda de Obsidian de 10.000 notas en 0.20.2 (notas inventadas, Mac M2 Pro con 16 GB, compilación de depuración, Claude Code escribiendo las páginas): se puede buscar a los 15 s, la primera página llega en unos 2 minutos y 8 páginas con citas en unos 4 minutos.

[Método de evaluación y detalles del benchmark](docs/eval/README.md).

<a id="learn-more"></a>
<a id="whats-new"></a>

<a id="cola-offline-outbox"></a>

<br>

## Leer más

- [Primeros pasos e instalación](docs/setup-and-data.es-ES.md#installation)
- [Conectar una herramienta de IA](docs/setup-with-ai.md)
- [Fuentes e importaciones](docs/import-guide.md)
- [Archivos locales, copias y uso sin conexión](docs/setup-and-data.es-ES.md)
- [Privacidad](docs/PRIVACY.md)
- [Diagnóstico e incidencias](https://wenlan.app/docs/diagnostics-and-issue-reports)
- [Novedades de cada versión](CHANGELOG.md)
- [Índice completo de documentación](docs/README.md)

<br>

## Contribuir

Únete a la [comunidad de Discord](https://discord.gg/nmzVTTU3HG). Informa de errores o pide funciones en [GitHub issues](https://github.com/wenlan-app/wenlan/issues); son públicos, así que oculta secretos y datos personales.

Para contribuir con código o documentación, consulta [Contribuir](.github/CONTRIBUTING.md), [Seguridad](.github/SECURITY.md) y el [Código de Conducta](.github/CODE_OF_CONDUCT.md).

<a id="code-signing-policy"></a>

<br>

## Política de firma de código

Consulta los [detalles de firma y notarización por plataforma](docs/code-signing.md).

<a id="license"></a>

<br>

## Licencia

El runtime local, la CLI, el servidor MCP, los tipos compartidos y los plugins usan [Apache-2.0](LICENSE). La aplicación de escritorio y su frontend usan [AGPL-3.0-only](app/LICENSE); si ejecutas una versión modificada como servicio de red, debes ofrecer su código fuente a los usuarios.

<a id="acknowledgments"></a>

<br>

## Linaje y proyectos afines

Wenlan toma su nombre del Pabellón Wenlan y continúa las tradiciones de LLM-wiki y agent-memory: [la nota LLM-wiki de Karpathy](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f), [la propuesta de Rohitg00](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2) y [nashsu/llm_wiki](https://github.com/nashsu/llm_wiki).
