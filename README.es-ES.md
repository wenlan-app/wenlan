<!-- README_SYNC: source=README.md sha256=b2887d5aa7c93f0e81477a0eaf2cb140997d0a682f02d4af5af87ee9c4acc631 -->

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

<!-- R113 walkthrough: 1080p GitHub delivery; original masters retained. -->
<p align="center">
  <a href="https://github.com/user-attachments/assets/6e55ae64-be35-4aae-8845-d5407b9ce3fc">
    <img src="./docs/assets/wenlan-personal-knowledge.png" alt="Notas, memorias y fuentes conectadas en Wenlan" width="100%">
  </a>
  <br><a href="https://github.com/user-attachments/assets/6e55ae64-be35-4aae-8845-d5407b9ce3fc">▶ Ver el vídeo</a>
</p>
<p>Composición de vistas reales con datos de ejemplo; el grafo es ilustrativo, no una vista nativa de tres columnas.</p>

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

### 1. Descarga y abre Wenlan

[Descarga la aplicación de escritorio](https://github.com/wenlan-app/wenlan/releases/latest) y ábrela:

- **macOS (Apple Silicon):** abre el `.dmg` y arrastra Wenlan a Aplicaciones. La aplicación está firmada y notarizada.
- **Windows x64:** ejecuta el `-setup.exe`; todavía no está firmado. Si SmartScreen avisa, confirma que procede de la página oficial de Releases antes de elegir «Más información» → «Ejecutar de todas formas».
- **Linux:** aún no hay aplicación de escritorio; sigue la [guía de configuración de IA](docs/setup-with-ai.md#install-the-runtime) para instalar el runtime.

Al iniciar por primera vez se descarga el modelo de búsqueda local. Mantén la conexión hasta terminar la configuración. [Detalles de descarga y privacidad](docs/PRIVACY.md#when-wenlan-reaches-the-network).

También puedes [usar Wenlan sin la aplicación de escritorio](docs/setup-and-data.es-ES.md#installation) mediante herramientas de IA o la CLI, con la misma base de conocimiento local.

### 2. Abre un ejemplo o crea una nota

Elige **Get started** para entrar en Wenlan. Explora el ejemplo de bienvenida o crea una nota propia. No hace falta importar nada ni configurar un modelo de IA.

### 3. Conecta una herramienta de IA cuando quieras

| Herramienta de IA | Conexión | Configuración |
|---|---|---|
| Claude Code | Plugin | [Guía de Claude Code](docs/setup-with-ai.md#claude-code) |
| Codex | Plugin | [Guía de Codex](docs/setup-with-ai.md#codex) |
| Cursor, Claude Desktop, VS Code, Gemini CLI, Muse Code | MCP | [Guía de MCP](docs/setup-with-ai.md#mcp-only-setup-and-other-clients) |

Tras instalar un plugin, inicia una nueva sesión de IA y ejecuta `/setup`. Conectar tu IA no requiere otro modelo de lenguaje local.

Prueba con la nota que acabas de abrir o guardar:

> Busca mi nota «[título]» en Wenlan y cítala.

**Primera prueba superada:** Tu IA encuentra esa nota y proporciona una cita que puedes abrir y comprobar.

<a id="daily-workflow"></a>

## Uso diario

Puedes usar la aplicación por sí sola o trabajar con tu IA. Guarda en Wenlan el conocimiento útil y el progreso para retomarlos en la próxima tarea.

<p align="center">
  <img src="./docs/assets/wenlan-daily-workflow-es-ES.png" alt="Las fuentes y memorias previas se acumulan en Wenlan; úsalas en la aplicación o con IA y guarda nuevas memorias y el progreso para continuar después." width="100%">
</p>

Flujo simplificado. Los comandos del plugin y la organización de notas son opcionales, sin un orden fijo.

- **Empieza por tu material.** Importa documentos y conversaciones seleccionadas como fuentes, o escribe notas en la aplicación. Allí puedes editar, consultar las fuentes citadas y revisar cambios sin conectar una IA.
- **Usa tu conocimiento y escribe notas nuevas.** Pide a tu IA que recupere fuentes y memorias relevantes con `/recall` y las use en la tarea. También puede escribir una nota con citas o actualizar una existente a partir de su versión actual y guardarla en Wenlan.
- **Conserva lo que aprendes.** Guarda una decisión o lección, junto con sus motivos y fuentes, en la aplicación o con `/capture` a través de tu IA.
- **Pausa y retoma.** `/handoff` guarda memorias, un registro de la sesión y los siguientes pasos; también actualiza las notas usadas durante la sesión si nuevos hallazgos las cambian. Usa `/brief` para leer el resumen del proyecto y retomar ese contexto la próxima vez.

**Organiza las memorias acumuladas cuando lo necesites:** `/distill` sintetiza o actualiza notas con citas a partir de memorias relacionadas; no es necesario para escribir una nota nueva. [Comandos del plugin](plugin/skills/README.md).

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

Las **fuentes** conservan el material que has leído; las **memorias** recogen tus conclusiones y el razonamiento que las sustenta. Reúne ambas en **notas Markdown con citas** que puedas leer, editar y enlazar mediante `[[wikilinks]]`. Sigue las citas para consultar el material original.

- **Importación de archivos y conversaciones:** Importa Markdown, texto, PDF con texto seleccionable, carpetas y ZIP de ChatGPT o Claude conservando las referencias a sus fuentes. Los PDF escaneados necesitan extracción de texto antes de importarlos. Los chats importados conservan una instantánea; las fuentes de archivos registradas se pueden actualizar desde el disco.
- **Conexión de solo lectura con Obsidian:** Lee una bóveda existente y actualízala bajo demanda sin modificar sus archivos ni sincronizar cambios de vuelta.
- **Memorias atómicas:** Recupera un hallazgo concreto sin cargar una conversación o nota entera.
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

**Las actualizaciones siguen las citas.** Si cambias o eliminas en Wenlan una fuente o memoria citada, el sistema marca para actualizar las notas que la utilizan. Con la IA en segundo plano activada y un modelo disponible, las notas aptas se reconstruyen a partir de las evidencias actuales; se rechazan los borradores con citas insuficientes.

- **Enriquecimiento en segundo plano con Steep:** Añade tipos, datos estructurados, pistas de búsqueda y enlaces al grafo a las memorias capturadas; la síntesis opcional combina memorias y fuentes en notas con citas.
- **[Revisión e historial](docs/knowledge-guide.es-ES.md#updates-and-history):** Consulta quién modificó cada nota, compara versiones y sigue las sustituciones de memorias. La actualización en segundo plano propone revisiones para las notas que hayas creado o editado; las notas configuradas para revisión también retienen los cambios explícitos de la IA hasta que los apruebes.
- **Exportación de wikis:** Exporta las notas aptas como una wiki Open Knowledge Format (OKF).
- **[Modelos opcionales en segundo plano](docs/setup-and-data.es-ES.md#models):** Activa el enriquecimiento, la síntesis de notas y la actualización automática con Qwen en el dispositivo (llama.cpp), servidores locales compatibles con OpenAI como Ollama o LM Studio, o proveedores de nube configurados.

<p align="center">
  <img src="./docs/assets/wenlan-updates-history.png" alt="Flujo ilustrativo: un hallazgo en un documento actualiza la memoria de V1 a V2 y después influye en una nota, que se actualiza automáticamente o queda pendiente de revisión." width="100%">
</p>
<p>Ilustración basada en el comportamiento actual de actualización; no es una captura de la app.</p>

<a id="architecture"></a>

## Arquitectura

**Una base local compartida.** La app, la CLI y las herramientas de IA conectadas leen y escriben mediante el servicio local de Wenlan. Las notas son archivos Markdown; memorias, relaciones e índices de búsqueda residen en libSQL local.

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
