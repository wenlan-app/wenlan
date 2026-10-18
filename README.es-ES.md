<!-- README_SYNC: source=README.md sha256=bb09d46a7e82483244078f7bdf6d1a8b0e078d9323b7360707e9443a5260dad1 -->

<p align="center">
  <picture>
    <source media="(max-width: 600px)" srcset="./docs/assets/readme-banner-es-ES-mobile.png">
    <img src="./docs/assets/readme-banner-es-ES.png" alt="Wenlan: Una wiki personal viva. La IA organiza. Tú tienes el control." width="100%">
  </picture>
</p>

Wenlan convierte tus documentos, archivos Markdown y conversaciones con IA en notas editables con enlaces a sus fuentes, para que tú y tus herramientas de IA podáis seguir trabajando a partir de ellas.

Cuando las fuentes cambian, puedes pedirle a la IA que actualice las notas o activar las actualizaciones en segundo plano. Si has editado una nota, Wenlan te propone cambios para que los revises, en lugar de sobrescribir tu trabajo automáticamente.

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-Hans.md">简体中文</a> | <a href="./README.zh-Hant.md">繁體中文</a> | Español
</p>

<p align="center">
  <a href="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml?query=branch%3Amain"><img alt="CI" src="https://github.com/wenlan-app/wenlan/actions/workflows/ci.yml/badge.svg?branch=main&event=push"></a>
  <a href="https://github.com/wenlan-app/wenlan/releases/latest"><img alt="Última versión" src="https://img.shields.io/github/v/release/wenlan-app/wenlan?sort=semver&label=release"></a>
  <a href="#license"><img alt="Licencia: Apache-2.0 y AGPL-3.0" src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20AGPL--3.0-blue.svg"></a>
</p>

<p align="center">
  <a href="#start-in-30-seconds">Primeros&nbsp;pasos</a> ·
  <a href="#what-does-wenlan-build">¿Qué&nbsp;es&nbsp;esto?</a> ·
  <a href="#what-can-it-do">Capacidades</a> ·
  <a href="#how-does-it-work">Flujo&nbsp;diario</a> ·
  <a href="#evaluation">Evaluación</a> ·
  <a href="#learn-more">Leer&nbsp;más</a>
</p>

https://github.com/user-attachments/assets/35f06749-00e5-484d-a9f4-5e462de8d11e

<p align="center">
  <sub>El primer fotograma es una composición ilustrativa, no una vista nativa de tres paneles. El resto muestra la aplicación en uso, con notas y citas a sus fuentes.</sub>
</p>

<a id="quickstart"></a>
<a id="start-in-30-seconds"></a>

## Primeros pasos

<a id="start-with-the-app"></a>
<a id="open-the-wiki"></a>
<a id="desktop-app"></a>

### 1. Descarga y abre Wenlan

[Descarga la aplicación](https://github.com/wenlan-app/wenlan/releases/latest) y ábrela tras instalarla:

- **macOS (Apple Silicon):** abre el `.dmg` y arrastra Wenlan a Aplicaciones. La aplicación está firmada y notarizada.
- **Windows x64:** ejecuta el `-setup.exe`. Todavía no está firmado. Si SmartScreen muestra un aviso, confirma que lo has descargado de la página oficial de Releases antes de elegir "Más información" → "Ejecutar de todas formas".
- **Linux:** todavía no tiene versión de escritorio; sigue la [guía de configuración](docs/setup-with-ai.md#install-the-runtime) para usar Wenlan con tus herramientas de IA sin la aplicación.

Al iniciar por primera vez, Wenlan descarga un modelo para la búsqueda local. Mantén la conexión a internet hasta terminar la configuración. [Detalles de descarga y privacidad](docs/PRIVACY.md#when-wenlan-reaches-the-network).

<a id="claude-code-in-30-seconds"></a>

<a id="codex-plugin"></a>

<a id="mcp-setup"></a>
<a id="mcp-clients"></a>

### 2. Empieza con tus propias notas

Abre Wenlan y empieza con una nota propia. La pantalla de bienvenida también ofrece un ejemplo que puedes explorar primero. Puedes empezar a escribir sin importar material, conectar una herramienta de IA ni configurar un modelo de IA.

### 3. Importa fuentes cuando quieras

Cuando te venga bien, puedes [importar archivos Markdown o conversaciones exportadas de ChatGPT / Claude](#what-can-i-bring-in). Es opcional: puedes empezar con una nota propia.

### 4. Conecta herramientas de IA cuando quieras ayuda

Conecta Claude Code, Codex u otras herramientas de IA compatibles cuando quieras que usen el mismo conocimiento. Una IA conectada puede redactar notas sin instalar otro modelo de lenguaje local.

<details>
<summary>¿Necesitas ayuda? Pídesela a tu IA</summary>

Reinicia tu herramienta de IA si se te indica. Si necesitas ayuda con la configuración, pega esto en Claude Code, Codex u otra herramienta que pueda seguir una guía:

```text
Conecta esta herramienta de IA a Wenlan siguiendo:
https://raw.githubusercontent.com/wenlan-app/wenlan/main/docs/setup-with-ai.md

Reutiliza mi instalación de Wenlan si ya existe.
Configura solo esta herramienta y comprueba que puede guardar y recuperar
una memoria de prueba.
```

</details>

Para organizar un tema con una IA conectada, abre una conversación de trabajo que merezca la pena conservar y pídele:

> Organiza las conclusiones, los motivos y las fuentes sobre [tema] de esta conversación en una nota de Wenlan con citas a las fuentes.

También puedes pedirle que organice en una nota un tema del material que hayas importado.

Cuando esté lista, abre la nueva nota en Wenlan, pulsa una cita para consultar su fuente y añade tus propias ideas. La próxima vez que trabajes en ese tema, pídele a tu IA que lea esta nota antes de continuar.

La organización y las actualizaciones automáticas en segundo plano son opcionales y necesitan un [modelo configurado](#models-and-privacy). Para instalar desde la terminal, usar otras plataformas o actualizar, consulta la [guía de configuración](docs/setup-and-data.es-ES.md#installation).

¿Necesitas ayuda? [Guía de configuración](docs/setup-with-ai.md) · [Comunicar un problema](https://github.com/wenlan-app/wenlan/issues). Los issues son públicos: antes de compartir contenido, incluidos los registros, oculta secretos, credenciales, tokens de acceso, identificadores y contenido privado o personal.



<a id="what-does-wenlan-build"></a>
<a id="why-it-compounds"></a>

<a id="qué-es-esto"></a>

## Tu Wiki, para ti y tu IA

- **Reúne material disperso por temas.** Organiza documentos, archivos Markdown y conversaciones con IA en notas conectadas, con citas que puedes abrir y comprobar.
- **Para ti y para tu IA.** Lee y edita tus notas, o pide a Claude Code o Codex que las use para continuar el trabajo.
- **Mantén tu conocimiento sin empezar de cero.** Sigue los cambios en las fuentes, actualiza las notas relacionadas y conserva su historial. La IA propone las revisiones de las notas que has editado para que decidas si aceptarlas.

Tus notas son archivos Markdown locales que puedes leer, editar y llevarte contigo.

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-system-mobile.png">
    <img src="./docs/assets/wenlan-system.png" alt="Fuentes y memorias se reúnen en una nota de ejemplo sobre Agent Loop. Una lección concreta complementa las pautas de reintento: las pruebas pasaron, pero una línea aún tapaba una etiqueta en móvil. La revisión visual en escritorio y móvil pasa a formar parte de los criterios que Claude o Codex pueden reutilizar." width="100%">
  </picture>
</p>

<a id="what-wenlan-is-not"></a>

La ilustración muestra una regla de trabajo reutilizable, no resultados de clientes ni una prueba de generación automática de notas. [Cómo se conectan las fuentes, memorias y notas](docs/knowledge-guide.es-ES.md#sources-and-pages).

<a id="knowledge-graph"></a>

<a id="sigue-las-conexiones-que-hay-detrás-de-una-página"></a>

### Sigue las conexiones que hay detrás de una nota

El ejemplo de Agent Loop conecta una pauta de reintentos, una lección de una revisión de interfaz y una lista de aceptación que puedes reutilizar.

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-knowledge-network-mobile.png">
    <img src="./docs/assets/wenlan-knowledge-network.png" alt="Nota ilustrativa de Agent Loop que cita una guía de diseño de agentes y una revisión de interfaz; las Memorias mencionadas incluyen pautas de reintento y una lección de comprobación visual, enlazadas a una lista de aceptación reutilizable y a conceptos y herramientas relacionados." width="100%">
  </picture>
</p>

<a id="retrieval"></a>
<a id="recuperación-a-través-de-palabras-significado-y-conexiones"></a>

Sigue una nota hasta los conceptos y las fuentes relacionados. [Cómo funcionan el grafo y la búsqueda](docs/knowledge-guide.es-ES.md#graph-and-search).

<a id="what-makes-wenlan-distinct"></a>
<a id="why-is-wenlan-different"></a>
<a id="two-lifecycles"></a>
<a id="two-lifecycles-one-maintained-knowledge-system"></a>
<a id="dos-ciclos-de-vida-un-sistema-de-conocimiento-mantenido"></a>

### El conocimiento cambia. El historial permanece.

La nueva experiencia mejora tus notas sin ocultar decisiones anteriores. Este ejemplo cambia «las pruebas pasaron, así que la interfaz está lista» por «comprueba las pruebas y las vistas de escritorio y móvil», conservando el motivo y las pruebas del cambio.

<p align="center">
  <picture>
    <source media="(max-width: 900px)" srcset="./docs/assets/wenlan-lifecycle-mobile.png">
    <img src="./docs/assets/wenlan-lifecycle.png" alt="Cambio ilustrativo de regla: «las pruebas pasaron, así que la interfaz está lista» pasa a «comprueba las pruebas y las vistas de escritorio y móvil»; las pruebas pasaron, pero una línea tapaba una etiqueta en móvil. Durante la actualización automática, los cambios en una nota que hayas editado se proponen para que los revises." width="100%">
  </picture>
</p>

<a id="local-markdown"></a>
<a id="memoria-atómica"></a>
<a id="página-mantenida"></a>
<a id="markdown-local-que-funciona-con-obsidian"></a>

La revisión se aplica a las actualizaciones de la IA; la edición directa de archivos y la reconstrucción forzada siguen otras reglas. [Actualizaciones, revisión y archivos locales](docs/knowledge-guide.es-ES.md#updates-and-history).

### ¿Ya usas Obsidian con plugins de IA?

Puedes usar [Copilot](https://docs.obsidiancopilot.com/agent-mode-and-tools/) para leer y editar notas con IA, [Smart Connections](https://github.com/brianpetro/obsidian-smart-connections) para encontrar contenido relacionado y [Obsidian Git](https://github.com/Vinzent03/obsidian-git) para conservar el historial de versiones. Tienes mucha flexibilidad, pero debes decidir cómo combinar los plugins, cuándo organizar el material nuevo y cómo actualizar las notas anteriores.

Wenlan reúne la organización de notas por tema, el seguimiento de cambios en las fuentes, las actualizaciones y el historial de revisiones en un mismo flujo de trabajo, ahorrándote el trabajo de conectar y mantener esos pasos por tu cuenta. Puedes leer el conocimiento organizado o dejar que Claude Code, Codex y otras herramientas lo usen para continuar el trabajo.

Pide a tu IA conectada que actualice una nota o [configura un modelo](#models-and-privacy) para activar las actualizaciones en segundo plano. Si has editado una nota, Wenlan te propone la revisión para que decidas si aceptarla.

Conecta tu bóveda de Obsidian como fuente de solo lectura sin mover tus archivos. Wenlan no modifica la bóveda ni sincroniza los cambios de vuelta.


<a id="what-you-get"></a>
<a id="what-can-it-do"></a>
<a id="what-can-i-bring-in"></a>

## Capacidades

### Incorpora tu material

- **Conserva conversaciones útiles con IA:** Importa archivos ZIP de ChatGPT o Claude sin duplicar conversaciones ya importadas.
- **Incorpora tus archivos actuales:** Incorpora Markdown, texto o PDF con texto extraíble, por archivo o carpeta; conecta una bóveda de Obsidian como fuente. Los PDF escaneados requieren extraer el texto primero.
- **Captura rápida:** Guarda una idea o decisión directamente en la aplicación de escritorio sin abrir un chat con IA.
- **[Guarda decisiones con tu IA](https://wenlan.app/learn/ai-work-memory):** Pide a tu herramienta de IA que guarde decisiones, lecciones, correcciones, preferencias y hechos, con sus fuentes y los registros que sustituyen.
- **Importa otra wiki:** Importa una wiki OKF externa mediante la CLI o la API, conservando referencias y enlaces. No se admite reimportar las propias exportaciones OKF de Wenlan.

### Explora tu Wiki personal

- **[Notas editables con fuentes](https://wenlan.app/docs/source-backed-pages):** Convierte documentos y Memorias relacionados en Notas Markdown con citas y enlaces a otras Notas.
- **Tarjetas o listas:** Explora Notas, Entidades y Espacios con la vista que prefieras.
- **[Grafo de conocimiento](docs/technical-foundations.md#graph-data-and-entity-resolution):** Explora conexiones entre personas, proyectos, afirmaciones y las Memorias que las respaldan.

### Úsalo con tus herramientas de IA

- **[Continúa con otra herramienta de IA](https://wenlan.app/docs/architecture):** Una vez conectados, Claude Code, Codex y otros clientes MCP pueden usar las mismas Notas y Memorias locales que la aplicación de escritorio y la CLI.
- **[Busca por palabras y significado](docs/technical-foundations.md#retrieval-pipeline):** Combina coincidencias exactas con búsqueda semántica local; las conexiones del grafo pueden aportar contexto.
- **[Búsqueda avanzada opcional](docs/technical-foundations.md#optional-channels-and-defaults):** Busca en Notas y Memorias más detalladas, con reordenación opcional para afinar los resultados.
- **[Separa tus proyectos](https://wenlan.app/docs/spaces):** Usa Espacios para elegir qué conocimiento laboral, personal, de clientes o de repositorios busca tu IA.
- **Acceso web (experimental):** Conecta un cliente web de IA como Claude, ChatGPT o Muse a toda tu biblioteca, o a un Espacio que elijas, mientras tu ordenador está conectado. Actívalo en Ajustes con el interruptor de acceso web. Las consultas y los resultados pasan por un relay; consulta el [funcionamiento del acceso y sus límites de privacidad](docs/PRIVACY.md#pre-release-standalone-wenlan-relay-connector).
- **Conecta tus propias herramientas:** Envía texto preparado, contenido web o Memorias mediante la API HTTP local. Acepta contenido, no URL para descargar.

### Mantén el conocimiento al día

La organización y las actualizaciones de Notas en segundo plano son opcionales y requieren un [modelo configurado](#models-and-privacy).

- **Sincronización incremental:** Las Fuentes de archivos y carpetas rastrean cambios en segundo plano. Las bóvedas de Obsidian siguen siendo de solo lectura y se sincronizan bajo demanda.
- **[Organiza el conocimiento guardado](docs/technical-foundations.md#typed-memory-schema):** Un modelo configurado puede añadir tipos, detalles estructurados, fechas relevantes, etiquetas, pistas de búsqueda y enlaces del grafo a las Memorias.
- **Actualizaciones respaldadas por citas:** La actualización automática rechaza borradores con citas insuficientes. Las Notas generadas por IA pueden actualizarse; los cambios en las que has editado se proponen para revisión.
- **[Revisa cuando hace falta criterio](https://wenlan.app/docs/review-and-trust):** Revisa conflictos protegidos, cambios de Notas, fusiones de entidades y vocabulario nuevo.
- **Sigue el progreso:** Consulta el progreso y los bloqueos en Activity. La sincronización, Steep (el enriquecimiento en segundo plano) y las actualizaciones de Notas elegibles pueden continuar tras cerrar la ventana, si están configurados y el servicio local sigue activo.

### Conserva tus datos y el control

- **[Conocimiento local e inspeccionable](https://wenlan.app/learn/local-first-ai-memory):** Conserva Notas Markdown, citas, revisiones y exportaciones a Obsidian; las Memorias y el grafo se guardan en libSQL local.
- **Llévate tu Wiki:** Exporta las Notas elegibles de todos los Espacios como wiki OKF v0.2 desde Ajustes o la CLI. No es una copia de seguridad de toda la base de datos.
- **[Elige el modelo](docs/technical-foundations.md#model-roles):** La búsqueda base es local. Steep, el enriquecimiento y la síntesis opcionales en segundo plano, puede usar Qwen en el dispositivo, un endpoint local o un modelo en la nube; los proveedores remotos reciben el contenido necesario para sus tareas.
- **Diagnóstico y reparaciones revisadas:** [Doctor](https://wenlan.app/docs/diagnostics-and-issue-reports) y [lint](plugin/skills/lint/SKILL.md) detectan problemas sin reescribir el conocimiento. Para los casos compatibles, puedes previsualizar una reparación en la aplicación, aplicarla explícitamente y verificarla.

**Empieza con una nota que quieras conservar.** [Primeros pasos](#start-in-30-seconds). Si prefieres probarlo más adelante, guarda este repositorio con una estrella.


<a id="how-wenlan-works"></a>
<a id="how-does-it-work"></a>

## Flujo diario

Una vez conectada tu herramienta de IA, puedes pedirle lo que necesitas con tus propias palabras:

Con el plugin de Wenlan instalado, también puedes usar los comandos con barra que aparecen a continuación.

### Antes de empezar una tarea

> Busca lo que he guardado en Wenlan sobre [tema], incluidas las decisiones anteriores y sus fuentes.

Atajo: `/recall [tema]`

### Cuando llegues a una conclusión útil

> Guarda esta decisión y por qué la tomamos en Wenlan, con sus fuentes.

Atajo: `/capture [decisión, motivos y fuentes]`

<a id="cuando-un-tema-merezca-una-página"></a>

### Cuando un tema merezca una nota

> Crea o actualiza una nota de Wenlan sobre [tema] con el material que hemos guardado. Conserva las citas.

Para organizar el conocimiento guardado en notas: `/distill`

Abre la nota en Wenlan para leerla, editarla y consultar sus fuentes. La próxima vez, pide a tu IA que la use para continuar el trabajo.

### Al terminar la sesión

> Guarda los avances, las decisiones y las preguntas pendientes de esta sesión en Wenlan para continuar la próxima vez.

Atajo: `/handoff`

<details>
<summary>Comandos del plugin y mantenimiento</summary>

- **Lee el resumen del proyecto:** `/brief [tema]` lee el resumen del proyecto del Espacio actual; un tema opcional añade contexto relacionado de ese mismo Espacio.
- **Comprueba y revisa:** `/lint` comprueba el estado del conocimiento; `/curate` revisa capturas o revisiones pendientes.

Estos comandos están disponibles a través de los plugins de Wenlan. Otros clientes conectados usan las herramientas MCP equivalentes. La organización y las actualizaciones de Notas en segundo plano son opcionales y requieren un [modelo configurado](#models-and-privacy).

[Referencia completa de comandos](plugin/skills/README.md).

</details>

<a id="cola-offline-outbox"></a>

[Escrituras offline y reenvío desde la CLI](docs/setup-and-data.es-ES.md#offline-queue).

<a id="models-and-privacy"></a>

### Modelos y privacidad

Puedes usar tu IA conectada para escribir notas sin instalar otro modelo de lenguaje local. La organización y las actualizaciones automáticas en segundo plano son opcionales y requieren configurar un modelo.

- **La búsqueda es local.** El modelo de búsqueda se descarga al iniciar por primera vez y se ejecuta en tu equipo, sin clave de API.
- **La IA conectada puede recibir tus conocimientos.** Un cliente de IA en la nube puede enviar el contenido recuperado a su proveedor. Si eliges un modelo en la nube para organizar material, también recibirá lo necesario para esa tarea. El almacenamiento local no convierte esas interacciones en locales.
- **Las estadísticas de uso están desactivadas por defecto.** Si las activas, Wenlan envía recuentos limitados de operaciones, versión y plataforma, no tus conocimientos ni un identificador de instalación. [Detalles](docs/PRIVACY.md#telemetry).

Consulta la [información de red y privacidad](docs/PRIVACY.md#when-wenlan-reaches-the-network) para las descargas, comprobaciones de actualizaciones, imágenes remotas y acceso remoto opcional.

[Opciones y configuración de modelos](docs/setup-and-data.es-ES.md#models).

### Tus datos y la desinstalación

Tus notas y registros de sesión son archivos Markdown; las memorias y el grafo se guardan en una base de datos local. Puedes conservar tus conocimientos al desinstalar la app.

[Ubicación de archivos, copias de seguridad y eliminación](docs/setup-and-data.es-ES.md#backup-and-removal).


<a id="evaluation"></a>

## Evaluación

Esto es una instantánea de solo recuperación, no una afirmación sobre la calidad de la respuesta de extremo a extremo. El método, los recibos del entorno y el flujo de actualización residen en [docs/eval](docs/eval/README.md).

<!-- EVAL_SNAPSHOT_START -->
| Benchmark | Recall@5 | MRR | NDCG@10 |
|---|---:|---:|---:|
| LME_Oracle (500 Q) | 93.6% | 0.857 | 0.883 |
| LME_S (deep, 90 Q) | 87.7% | 0.815 | 0.822 |
<!-- EVAL_SNAPSHOT_END -->


<a id="learn-more"></a>

## Leer más

Documentación más detallada, conceptos y comparaciones:

### Documentación

- [Primeros pasos](https://wenlan.app/docs/get-started): instala y verifica el primer bucle local.
- [Flujo diario](https://wenlan.app/docs/daily-workflow): brief, capture, recall, handoff, distill, lint y curate.
- [Clientes MCP](https://wenlan.app/docs/mcp-clients): conecta Claude Code, Codex, Cursor, Claude Desktop, Muse Code y otros clientes.

### Guías de flujo de trabajo

- [Crear tu primera LLM Wiki (guía en inglés)](https://wenlan.app/learn/distilled-wiki-pages-ai-memory): prueba un ejemplo con tres fuentes, revisa las citas y comprueba qué cambia al actualizar una fuente.
- [Crear una base de conocimiento de proyectos para consultoría](https://wenlan.app/learn/build-client-project-knowledge-base-for-consulting)
- [Crear una base de conocimiento para investigación de inversiones](https://wenlan.app/learn/build-investment-research-knowledge-base)
- [Crear una base de conocimiento de investigación de producto antes de redactar un PRD](https://wenlan.app/learn/build-product-research-knowledge-base-for-prd)
- [Crear una base de conocimiento de incidentes SRE](https://wenlan.app/learn/build-sre-incident-knowledge-base)
- [Crear una base de conocimiento de definiciones de métricas de negocio](https://wenlan.app/learn/build-business-metric-definition-knowledge-base): convierte especificaciones de KPI aprobadas en un diccionario de datos respaldado por fuentes, con texto de fórmula, granularidad, exclusiones, propietarios, revisiones y estado de revisión.

### Conceptos

- [Por qué una wiki viva, no solo memoria de IA](https://wenlan.app/learn/ai-work-memory): el problema y el modelo de producto en profundidad.
- [Servidor de memoria MCP](https://wenlan.app/learn/mcp-memory-server): cómo Wenlan expone el conocimiento a través de herramientas de IA.
- [Memoria de IA local-first](https://wenlan.app/learn/local-first-ai-memory): datos, privacidad y control.
- [Markdown e índice local](https://wenlan.app/learn/local-first-ai-memory): almacenamiento, recuperación y propiedad.
- [Bucle de entrega de agentes de IA](https://wenlan.app/learn/ai-agent-handoff-loop): cómo trasladar el trabajo limpiamente a la siguiente sesión.
- [Base de conocimiento para investigación](https://wenlan.app/learn/source-backed-research-knowledge-base): convierte artículos seleccionados en una matriz bibliográfica y una síntesis verificable con fuentes.

### Comparaciones

- [Wenlan vs Basic Memory](https://wenlan.app/learn/wenlan-vs-basic-memory)
- [Wenlan vs claude-mem](https://wenlan.app/learn/wenlan-vs-claude-mem)
- [Wenlan vs Superlocal Memory](https://wenlan.app/learn/wenlan-vs-superlocal-memory)


## Contribuir

Las correcciones de errores, casos de evaluación, documentación y funciones son bienvenidos. Instalar Wenlan no requiere compilar desde el código fuente. Para el desarrollo local, ejecuta estos comandos desde la raíz de este repositorio:

```bash
# crates del daemon (default-members: la aplicación de escritorio no se compila)
cargo build
cargo test

# aplicación de escritorio (target de Cargo y herramientas frontend en la raíz)
pnpm install
pnpm dev:all
pnpm build:all
```

`pnpm dev:all` es el punto de entrada de desarrollo admitido para la aplicación de escritorio. Mantiene los puertos de desarrollo, los datos, la propiedad de los procesos, la identidad de la aplicación, los sockets MCP y el estado de Remote Access separados del runtime de producción instalado; una compilación de depuración iniciada sin ese aislamiento se niega a ejecutarse. Consulta el [AGENTS.md](AGENTS.md) y [CONTRIBUTING.md](.github/CONTRIBUTING.md) de este repositorio, además del [app/AGENTS.md](app/AGENTS.md) dentro del repositorio, para el flujo de trabajo de desarrollo completo. Reportes de seguridad: [SECURITY.md](.github/SECURITY.md). Política de privacidad: [PRIVACY.md](docs/PRIVACY.md). Por favor, lee también el [Código de Conducta](.github/CODE_OF_CONDUCT.md).


<a id="code-signing-policy"></a>

## Política de firma de código

Las versiones de escritorio para macOS están firmadas con Developer ID y notarizadas. Los instaladores de Windows aún no están firmados.

Las versiones se compilan desde commits etiquetados mediante el workflow de release de este repositorio, en runners alojados por GitHub. Los mantenedores deben usar autenticación multifactor en GitHub.

[Detalles de firma por plataforma (en inglés)](docs/code-signing.md) · [Política de privacidad (en inglés)](docs/PRIVACY.md).


<a id="license"></a>

## Licencia

Wenlan usa dos licencias, una por cada parte del repositorio.

- **Apache-2.0** ([`LICENSE`](LICENSE)) cubre el runtime local, la CLI, el servidor MCP, los tipos compartidos y los archivos del plugin de Claude Code y Codex. Constrúyelo libremente sobre esto.
- **AGPL-3.0-only** ([`app/LICENSE`](app/LICENSE)) cubre la aplicación de escritorio: el crate `app/` y el frontend de React que incluye. Si ejecutas una versión modificada de la aplicación como servicio en red, la AGPL te pide ofrecer ese código modificado a sus usuarios.

La separación es deliberada. El código Apache-2.0 puede usarse dentro de un programa AGPL-3.0, así que la aplicación de escritorio se apoya en el runtime sin que ninguna de las dos licencias se incumpla.


<a id="acknowledgments"></a>

## Linaje y pares

Wenlan (文瀾) toma su nombre de Wenlan Ge (文瀾閣), una biblioteca imperial que albergaba la Siku Quanshu como parte de una de las colecciones de libros más grandes de China.

El modelo llm-wiki v2 de Wenlan es su propia dirección de producto, informada por los linajes de LLM-wiki y memoria de agentes:

- La [nota de LLM-wiki de Karpathy](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) estableció el patrón de fuente bruta a wiki mantenida.
- La [propuesta de LLM Wiki v2 de Rohitg00](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2) extiende ese patrón con ciclo de vida de memoria, confianza, grafo y mecanismos de recuperación. [agentmemory](https://github.com/rohitg00/agentmemory) es su implementación concreta de memoria de agente.
- [nashsu/llm_wiki](https://github.com/nashsu/llm_wiki) es una implementación completa de escritorio del patrón LLM-wiki centrado en documentos.
- [basic-memory](https://github.com/basicmachines-co/basic-memory), [obsidian-mind](https://github.com/breferrari/obsidian-mind), [mcp-memory-service](https://pypi.org/project/mcp-memory-service/), [Memoria](https://github.com/matrixorigin/Memoria) y [OpenMemory](https://github.com/CaviraOSS/OpenMemory) exploran formas adyacentes de conocimiento local y memoria de agentes.
