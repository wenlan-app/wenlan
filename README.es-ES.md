<!-- README_SYNC: source=README.md sha256=242205b349b100ea70d6d1cbe0e6c7c396df7b9f2f6bcf922b817f3d9b9e0866 -->

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-es-ES-mobile.png">
    <img src="./docs/assets/readme-banner-es-ES.png" alt="Wenlan: Una wiki personal viva" width="100%">
  </picture>
</p>

<p align="center">
  Wenlan organiza notas, documentos y conversaciones dispersos en una base de conocimiento que se actualiza y enlaza a sus fuentes,<br>
  para que tú y herramientas de IA como Claude y Codex reutilicéis lo aprendido sin tener que explicar de nuevo decisiones anteriores.
</p>

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-Hans.md">简体中文</a> | <a href="./README.zh-Hant.md">繁體中文</a> | Español
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml?query=branch%3Amain"><img alt="CI" src="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml/badge.svg?branch=main&event=push"></a>
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><img alt="Última versión" src="https://img.shields.io/github/v/release/wenlan-app/wenlan?label=release"></a>
  <a href="#license"><img alt="Licencia: Apache-2.0 y AGPL-3.0" src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20AGPL--3.0-blue.svg"></a>
</p>

<p align="center">
  <a href="#start-in-30-seconds">Primeros&nbsp;pasos</a> ·
  <a href="#what-can-i-bring-in">Funciones</a> ·
  <a href="#learn-more">Docs</a> ·
  <a href="https://discord.gg/nmzVTTU3HG">Discord</a>
</p>

<!-- R113 walkthrough: approved 0.5-second cover; 1080p; original video/audio and masters retained. -->

https://github.com/user-attachments/assets/5e661389-d7ea-4a36-a645-2b1ec2a92d32

<p>Portada: composición de vistas reales con datos de ejemplo; el grafo es ilustrativo, no una vista nativa de tres columnas.</p>

<a id="quickstart"></a>
<a id="start-in-30-seconds"></a>
<a id="start-with-the-app"></a>
<a id="open-the-wiki"></a>
<a id="desktop-app"></a>
<a id="claude-code-in-30-seconds"></a>
<a id="codex-plugin"></a>
<a id="mcp-setup"></a>
<a id="mcp-clients"></a>

## Primeros pasos

Guarda tu material en Wenlan para que tus herramientas de IA habituales lo usen para responder preguntas y ayudarte a completar tareas.

### 1. Descarga y abre Wenlan

[Descarga la aplicación de escritorio](https://github.com/wenlan-app/wenlan/releases/latest) (macOS Apple Silicon o Windows x64), instálala y ábrela.

El primer inicio requiere internet para descargar el modelo de búsqueda local. Por separado, un [modelo opcional para Steep](docs/setup-and-data.es-ES.md#models) permite organizar recuerdos y actualizar páginas para que tu IA reutilice ese contexto.

[Instalación y otras formas de usar Wenlan](docs/setup-and-data.es-ES.md#installation) · [Privacidad](docs/PRIVACY.md)

### 2. Guarda tu primera página

Elige **Get started** para entrar en Wenlan; después, crea o importa una página y guárdala. También puedes explorar el ejemplo de bienvenida.

### 3. Conecta una herramienta de IA cuando quieras

| Herramienta de IA | Conexión | Configuración |
|---|---|---|
| Claude Code | Plugin | [Guía de Claude Code](docs/setup-with-ai.md#claude-code) |
| Codex | Plugin | [Guía de Codex](docs/setup-with-ai.md#codex) |
| Cursor, Claude Desktop, VS Code, Gemini CLI, Muse Code | MCP | [Guía de MCP](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |

Tras instalar un plugin, inicia una nueva sesión de IA y ejecuta `/setup`. Conectar tu IA no requiere otro modelo de lenguaje local.

Después, pídele a tu IA:

> Usa la página «Título de la página» de Wenlan para ayudarme con esta tarea: …

<a id="daily-workflow"></a>

## Uso diario

Guarda los nuevos hallazgos y avances mientras trabajas y retoma donde lo dejaste la próxima vez.

<p align="center">
  <img src="./docs/assets/wenlan-daily-workflow-es-ES.png" alt="Las fuentes y memorias previas se acumulan en Wenlan; úsalas en la aplicación o con IA y guarda nuevas memorias y el progreso para continuar después." width="100%">
</p>

Flujo simplificado. Los comandos del plugin y la organización de páginas son opcionales, sin un orden fijo.

- **Empieza por tu material.** Importa documentos y conversaciones seleccionadas como fuentes, o escribe páginas en la aplicación. Allí puedes editar, consultar las fuentes citadas y revisar cambios sin conectar una IA.
- **Usa tu conocimiento y escribe páginas nuevas.** Pide a tu IA que recupere fuentes y memorias relevantes para la tarea (`/recall`). También puede escribir una página con citas o actualizar una existente a partir de su versión actual y guardarla en Wenlan.
- **Conserva lo que aprendes.** Guarda una decisión o lección, junto con sus motivos y fuentes, en la aplicación o a través de tu IA (`/capture`).
- **Pausa y retoma.** Guarda memorias, un registro de la sesión y los siguientes pasos, y actualiza las páginas usadas si nuevos hallazgos las cambian (`/handoff`). La próxima vez, carga el resumen del proyecto para retomar el trabajo (`/brief`).

**Organiza las memorias acumuladas cuando lo necesites:** Sintetiza o actualiza páginas con citas a partir de memorias relacionadas (`/distill`); no es necesario para escribir una página nueva. [Comandos del plugin](plugin/skills/README.md).

<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>
<a id="qué-es-esto"></a>
<a id="what-is-this"></a>
<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="what-can-i-bring-in"></a>
<a id="knowledge-graph"></a>

<a id="explora-y-organiza-tu-conocimiento"></a>

<a id="conecta-tu-conocimiento"></a>

## Organiza la información dispersa en conocimiento

Las **fuentes** conservan el material que has leído; las **memorias** recogen tus conclusiones y el razonamiento que las sustenta. Reúne ambas en **páginas Markdown con citas** que puedas leer, editar y enlazar mediante `[[wikilinks]]`. Sigue las citas para consultar el material original.

- **Importación de archivos y conversaciones:** Importa Markdown, texto, PDF con texto seleccionable, carpetas y ZIP de ChatGPT o Claude conservando las referencias a sus fuentes. Los PDF escaneados necesitan extracción de texto antes de importarlos. Los chats importados conservan una instantánea; las fuentes de archivos registradas se pueden actualizar desde el disco.
- **Conexión de solo lectura con Obsidian:** Lee una bóveda existente y actualízala bajo demanda sin modificar sus archivos ni sincronizar cambios de vuelta.
- **Memorias atómicas:** Recupera un hallazgo concreto sin cargar una conversación o página entera.
- **Grafo de conocimiento:** Descubre temas relacionados mediante entidades tipadas, relaciones y grupos de conocimiento (comunidades); un modelo de enriquecimiento configurado vincula menciones repetidas a la misma entidad (resolución de menciones).

<a id="explora-tu-grafo-de-conocimiento-personal"></a>

<p align="center">
  <img src="./docs/assets/wenlan-personal-graph.png" alt="Grafo real de Wenlan con regiones ilustrativas de intereses personales, investigación y desarrollo" width="100%">
</p>
<p>Nodos y conexiones de la app; los nombres y límites de las regiones son ilustrativos.</p>

<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>

<a id="what-wenlan-is-not"></a>
<a id="retrieval"></a>
<a id="recuperación-a-través-de-palabras-significado-y-conexiones"></a>
<a id="retrieval-across-words-meaning-and-connections"></a>

<a id="usa-tu-conocimiento-con-ia"></a>

## Pon tu conocimiento a trabajar con IA

Usa la misma base de conocimiento con distintas herramientas de IA. Los **plugins** indican cuándo recuperar contexto anterior y guardar decisiones nuevas; las **conexiones solo por MCP** permiten que tu IA use estas herramientas cuando las necesite o se lo pidas.

- **[Búsqueda híbrida local](docs/technical-foundations.md#retrieval-pipeline):** Encuentra términos exactos y significados similares con SQLite FTS5 y embeddings BGE, combinados mediante Reciprocal Rank Fusion (RRF) ponderado; la búsqueda base no requiere clave API.
- **Clasificación asistida por el grafo:** Prioriza memorias relacionadas cuando hay enlaces entre entidades, con reranking cross-encoder opcional para afinar la relevancia.
- **Búsqueda por Espacio:** Selecciona un Espacio para limitar el conocimiento devuelto al proyecto o colección personal pertinente.

<p align="center">
  <img src="./docs/assets/wenlan-knowledge-to-work-es-ES.png" alt="La IA continúa con lo que ya sabes: búsqueda por palabras clave, significado y grafo" width="100%">
</p>
<p>Ejemplo ficticio, no una prueba del producto; Wenlan aporta contexto junto con la memoria nativa de Claude.</p>

Activa el interruptor **Web access** en Settings para que clientes web de IA compatibles, como Claude, ChatGPT o Muse, puedan buscar en toda la biblioteca o en un Espacio elegido mientras el ordenador con Wenlan esté conectado. Esta función es experimental; las consultas y los resultados pasan por un relay. Consulta los [límites de privacidad](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector).

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

## Actualiza el conocimiento, conserva su historial

**Las actualizaciones siguen las citas.** Si cambias o eliminas en Wenlan una fuente o memoria citada, el sistema marca para actualizar las páginas que la utilizan. Con la IA en segundo plano activada y un modelo disponible, las páginas aptas se reconstruyen a partir de las evidencias actuales; se rechazan los borradores con citas insuficientes.

- **Enriquecimiento en segundo plano con Steep:** Añade tipos, datos estructurados, pistas de búsqueda y enlaces al grafo a las memorias capturadas; la síntesis opcional combina memorias y fuentes en páginas con citas.
- **[Revisión e historial](docs/knowledge-guide.es-ES.md#updates-and-history):** Consulta quién modificó cada página, compara versiones y sigue las sustituciones de memorias. La actualización en segundo plano propone revisiones para las páginas que hayas creado o editado; las páginas configuradas para revisión también retienen los cambios explícitos de la IA hasta que los apruebes.
- **Exportación de wikis:** Exporta las páginas aptas como una wiki Open Knowledge Format (OKF).
- **[Modelos opcionales en segundo plano](docs/setup-and-data.es-ES.md#models):** Activa el enriquecimiento, la síntesis de páginas y la actualización automática con Qwen en el dispositivo (llama.cpp), servidores locales compatibles con OpenAI como Ollama o LM Studio, o proveedores de nube configurados.

<p align="center">
  <img src="./docs/assets/wenlan-updates-history.png" alt="Flujo ilustrativo: un hallazgo en un documento actualiza la memoria de V1 a V2 y después influye en una nota, que se actualiza automáticamente o queda pendiente de revisión." width="100%">
</p>
<p>Ilustración basada en el comportamiento actual de actualización; no es una captura de la app.</p>

<a id="architecture"></a>

## Arquitectura

**Una base local compartida.** La app, la CLI y las herramientas de IA conectadas leen y escriben mediante el servicio local de Wenlan. Las páginas son archivos Markdown; memorias, relaciones e índices de búsqueda residen en libSQL local.

<p align="center">
  <img src="./docs/assets/wenlan-architecture-es-ES.png" alt="Vista de la arquitectura: la app y las herramientas de IA comparten una base de conocimiento local, con modelos opcionales para la organización en segundo plano." width="100%">
</p>

Esquema de la arquitectura. [Búsqueda, evidencias y modelos](docs/technical-foundations.md).

<a id="models-and-privacy"></a>

## Datos y privacidad

Un cliente de IA en la nube puede enviarle el conocimiento recuperado a su proveedor; un modelo en la nube también recibe el contenido que procesa. El almacenamiento local no mantiene locales esas interacciones. Las estadísticas de uso están desactivadas por defecto.

[Ubicación de archivos, copias y eliminación](docs/setup-and-data.es-ES.md#backup-and-removal) · [Red y privacidad](docs/PRIVACY.md#when-wenlan-reaches-the-network).

<a id="evaluation"></a>

## Evaluación

Estas mediciones de LongMemEval (LME) evalúan la recuperación de conocimiento guardado, no la calidad de las respuestas de IA. Recall@5 mide cuánto material relevante aparece entre los cinco primeros resultados.

<!-- EVAL_SNAPSHOT_START -->
| Benchmark | Recall@5 | MRR | NDCG@10 |
|---|---:|---:|---:|
| LME_Oracle (500 Q) | 93.6% | 0.857 | 0.883 |
| LME_S (deep, 90 Q) | 87.7% | 0.815 | 0.822 |
<!-- EVAL_SNAPSHOT_END -->

[Método de evaluación y detalles del benchmark](docs/eval/README.md).

<a id="learn-more"></a>

<a id="cola-offline-outbox"></a>

## Leer más

- [Primeros pasos e instalación](docs/setup-and-data.es-ES.md#installation)
- [Conectar una herramienta de IA](docs/setup-with-ai.md)
- [Fuentes e importaciones](docs/knowledge-guide.es-ES.md#sources-and-pages) · [Archivos locales, copias y uso sin conexión](docs/setup-and-data.es-ES.md)
- [Privacidad](docs/PRIVACY.md)
- [Diagnóstico e incidencias](https://wenlan.app/docs/diagnostics-and-issue-reports)
- [Índice completo de documentación](docs/README.md)

## Contribuir

Únete a la [comunidad de Discord](https://discord.gg/nmzVTTU3HG). Informa de errores o pide funciones en [GitHub issues](https://github.com/wenlan-app/wenlan/issues); son públicos, así que oculta secretos y datos personales. Para contribuir con código o documentación, consulta [Contribuir](.github/CONTRIBUTING.md), [Seguridad](.github/SECURITY.md) y el [Código de Conducta](.github/CODE_OF_CONDUCT.md).

<a id="code-signing-policy"></a>

## Política de firma de código

Consulta los [detalles de firma y notarización por plataforma](docs/code-signing.md).

<a id="license"></a>

## Licencia

El runtime local, la CLI, el servidor MCP, los tipos compartidos y los plugins usan [Apache-2.0](LICENSE). La aplicación de escritorio y su frontend usan [AGPL-3.0-only](app/LICENSE); si ejecutas una versión modificada como servicio de red, debes ofrecer su código fuente a los usuarios.

<a id="acknowledgments"></a>

## Linaje y proyectos afines

Wenlan toma su nombre del Pabellón Wenlan y continúa las tradiciones de LLM-wiki y agent-memory: [la nota LLM-wiki de Karpathy](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f), [la propuesta de Rohitg00](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2) y [nashsu/llm_wiki](https://github.com/nashsu/llm_wiki).
