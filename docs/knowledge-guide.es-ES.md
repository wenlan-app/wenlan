# Guía de conocimiento: fuentes, memorias, páginas y búsqueda

Esta guía explica cómo Wenlan conecta el material de origen, las memorias, las páginas, las relaciones del grafo, la búsqueda, las actualizaciones, la revisión y los archivos locales.

Idiomas: [English](knowledge-guide.md) · [繁體中文](knowledge-guide.zh-Hant.md) · [简体中文](knowledge-guide.zh-Hans.md) · [Español](knowledge-guide.es-ES.md)<br>
[Volver al README en español](../README.es-ES.md)

Contenido: [Fuentes y páginas](#sources-and-pages) · [Grafo y búsqueda](#graph-and-search) · [Actualizaciones e historial](#updates-and-history)

<a id="sources-and-pages"></a>

## Fuentes y páginas

El ejemplo del README adapta una [guía de diseño de agentes](https://www.anthropic.com/engineering/building-effective-agents) y las [correcciones de los diagramas de este README](https://github.com/wenlan-app/wenlan/commit/061fbc6ab12a76ec805869e46d464a58a06db296). Ilustra conocimiento reutilizable, no resultados medidos de clientes ni una página generada automáticamente. La ilustración está en el [README en español](../README.es-ES.md#what-does-wenlan-build).

Con Wenlan, el trabajo en curso no se queda dentro de la ventana del chat. Puedes guardar los documentos y conversaciones que elijas, anotar las decisiones que tomes y organizarlos en páginas que puedas leer, editar y reutilizar. Puedes redactar una página directamente con tu IA conectada; la síntesis opcional basada en modelos y el mantenimiento en segundo plano requieren un [modelo configurado](setup-and-data.es-ES.md#models).

<a id="what-wenlan-is-not"></a>

**Pensado para trabajos que continúan.** Si usas la IA durante días o semanas para el mismo tema y tienes que buscar continuamente el material anterior o volver a explicar decisiones ya tomadas, Wenlan está pensado para ese flujo de trabajo. No es un sistema de gestión personal ni un SDK de memoria integrado en otro producto. Puedes seguir usando Obsidian; Wenlan no promete sustituir sus plugins ni migrar todas las funciones de tu vault.

**Un sistema de conocimiento, tres roles:**

- **Las Fuentes mantienen rastreable el material que lee Wenlan.** Las conversaciones importadas permanecen como registros capturados; los archivos registrados sincronizan su contenido actual a medida que cambian.
- **Las Memorias preservan lo que el trabajo te enseña.** Los agentes capturan decisiones atómicas, lecciones, correcciones y sustituciones con procedencia.
- **Las Páginas compilan el conocimiento actual.** Wenlan convierte Fuentes y Memorias relevantes en Markdown con citas de fuente que puedes reutilizar, actualizar y revisar.

**Cómo se actualizan las páginas:** Tanto las Fuentes como las Memorias capturadas pueden sustentar una misma Página. El historial de Memoria registra los cambios en cada Memoria; el historial de la Página registra las fuentes que la sustentan y sus revisiones. Durante la actualización automática, las Páginas elegibles mantenidas por el sistema pueden actualizarse directamente; las que hayas editado reciben una revisión propuesta. La revisión te permite decidir si aplicarla; no garantiza que la conclusión de la IA sea correcta.

Para lectores técnicos: Wenlan sigue el patrón de las **wikis para LLM**. Consulta la [guía de implementación de LLM wiki](https://wenlan.app/learn/distilled-wiki-pages-ai-memory) y los [fundamentos técnicos](technical-foundations.md) para conocer el modelo de datos, la recuperación y las reglas de mantenimiento.

<a id="graph-and-search"></a>

## Grafo y búsqueda

El ejemplo de Agent Loop conecta una pauta de reintentos, una lección de una revisión de interfaz y una lista de aceptación que puedes reutilizar.

El grafo de entidad-relación es una parte del conjunto más amplio de páginas conectadas de Wenlan. Las **Páginas de Conocimiento** contienen la síntesis mantenida, las **Entidades** anclan personas, proyectos y conceptos reutilizables, las **Páginas de Fuente** hacen que el material importado o sincronizado sea inspeccionable, y las **Memorias** atómicas preservan decisiones y cambios. Funcionan a través de enlaces separados y explícitos: wikilinks de Página a Página, evidencia de Página, enlaces de Memoria a Entidad y relaciones de Entidad dirigidas.

Dentro del grafo de entidades, un modelo de enriquecimiento configurado extrae Entidades tipadas, observaciones y relaciones dirigidas a partir de las Memorias. El enlace y la resolución de entidades reutilizan nodos existentes en lugar de tratar cada mención como nueva; cada Memoria conserva su Fuente y puede vincularse a múltiples Entidades. [Cómo se almacena el modelo conectado ->](technical-foundations.md#connected-knowledge-model)

- **Significado y dirección:** Las relaciones utilizan un vocabulario predefinido como `uses` (usa), `part_of` (parte de), `contradicts` (contradice) y `replaced_by` (reemplazado por); los tipos desconocidos se reasignan a `related_to` (relacionado con) y se convierten en propuestas de vocabulario revisables.
- **Fuerza y procedencia:** Una relación puede almacenar confianza, una explicación y su Memoria de origen, para que las afirmaciones más fuertes y más débiles sigan siendo distinguibles e inspeccionables.
- **Comunidades que se enriquecen con el tiempo:** La propagación de etiquetas agrupa Entidades por densidad de relación, ponderada por el recuento de relaciones entre cada par. Estos grupos pueden organizar resúmenes de corpus opcionales mientras que los enlaces de Entidad añaden contexto de recuperación.
- **Corrección sin borrado:** Las afirmaciones relacionadas, las correcciones y las sustituciones explícitas permanecen inspeccionables juntas mientras se conservan las Fuentes originales y el historial de Memoria.

Durante la recuperación, la coincidencia densa de entidades encuentra entidades relevantes para la consulta. Cuando existen enlaces de grafo elegibles, el flujo predeterminado de grafo-memoria potencia las Memorias vinculadas como una tercera señal de [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf). La ruta depende de los datos y el alcance, y los límites de Espacio (Space) se siguen aplicando. [Cómo funciona la ruta del grafo ->](technical-foundations.md#graph-assisted-retrieval)

<a id="retrieval"></a>

### Recuperación a través de palabras, significado y conexiones

La búsqueda central de Wenlan es un pipeline híbrido local, no una simple búsqueda de vectores. Cada etapa tiene una tarea diferente:

- **Coincidencia literal, [SQLite FTS5](https://www.sqlite.org/fts5.html):** un índice de texto completo encuentra términos literales, identificadores y frases.
- **Significado similar, FastEmbed + [`Qdrant/bge-base-en-v1.5-onnx-Q`](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q):** un modelo inglés cuantizado crea embeddings de 768 dimensiones; [libSQL cosine DiskANN](https://turso.tech/blog/approximate-nearest-neighbor-search-with-diskann-in-libsql) los indexa para recuperar vecinos aproximados.
- **Clasificación combinada, [RRF](https://cormack.uwaterloo.ca/cormacksigir09-rrf.pdf) ponderado (`k = 60`):** las listas de clasificación léxica y semántica se fusionan sin fingir que sus puntuaciones brutas comparten una escala; la similitud de coseno también pondera la contribución del vector.
- **Contexto conectado, flujo de grafo-memoria:** los enlaces de entidad elegibles añaden una tercera señal RRF mientras que el alcance de lectura activo sigue filtrando las Memorias devueltas.
- **Precisión opcional, re-clasificación por cross-encoder:** a diferencia de los embeddings, [`jinaai/jina-reranker-v1-turbo-en`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en) o [`BAAI/bge-reranker-base`](https://huggingface.co/BAAI/bge-reranker-base) lee cada par consulta-candidato y reordena el grupo más pequeño; la re-clasificación está desactivada por defecto.

Los canales de Página, episódicos y de hechos son opcionales y recurren a las señales de búsqueda restantes si no están disponibles. El Espacio sigue limitando el alcance de lectura. [Métodos, valores predeterminados y limitaciones ->](technical-foundations.md)

<a id="updates-and-history"></a>

## Actualizaciones e historial

Las páginas generadas pueden quedar obsoletas; un almacén de memoria puede fragmentarse en hechos desconectados. Wenlan vincula dos ciclos de vida sin colapsarlos en una sola capa.

### Memoria Atómica

`CAPTURA -> CLASIFICA -> ENRIQUECE -> VINCULA -> RECONCILIA`

La captura y la sustitución explícita son fundamentales. Las etapas basadas en modelos se ejecutan solo cuando el modelo correspondiente está configurado, y el paso de reconciliación está desactivado por defecto.

| Operación | Lo que hace Wenlan |
|---|---|
| **Captura** | Los agentes escriben una idea completa y autónoma por Memoria, siguiendo el principio de nota atómica de Zettelkasten en lugar de guardar toda la conversación. |
| **Clasifica** | Con un modelo de lenguaje configurado, Wenlan asigna `identity` (identidad), `preference` (preferencia), `decision` (decisión), `lesson` (lección), `gotcha` (advertencia) o `fact` (hecho); un tipo preciso proporcionado por el cliente tiene prioridad. |
| **Enriquece** | Con un modelo de lenguaje configurado, añade campos estructurados, pistas de recuperación, fechas de eventos, calidad, importancia y etiquetas cuando estén disponibles. |
| **Vincula** | Mantiene la procedencia y, cuando el enriquecimiento está habilitado, conecta Memorias con entidades y relaciones en el grafo de conocimiento. |
| **Reconcilia** | Los reemplazos explícitos preservan una cadena de `supersedes` (sustituye). Un reemplazo de un agente cuyo nivel de confianza sea inferior a "full" se pone en cola para revisión humana automáticamente, sin necesidad de ninguna opción. Un paso opcional basado en un modelo también puede poner en cola conflictos protegidos para revisión en lugar de sobrescribir el historial; ese paso está desactivado por defecto y debe habilitarse explícitamente. |

Configuración avanzada: establece `WENLAN_ENABLE_DUAL_POOL_RESOLVE=1` para habilitar ese paso de reconciliación.

### Página Mantenida

`DESTILA -> CITA -> RASTREA -> ACTUALIZA -> REVISA`

| Operación | Lo que hace Wenlan |
|---|---|
| **Destila** | Compila Fuentes y Memorias relacionadas en una Página Markdown. |
| **Cita** | Mantiene los registros de citas y el estado de verificación; la actualización automática descarta un borrador cuando falla la verificación del respaldo de las citas. |
| **Rastrea** | Registra qué evidencia sustenta la Página, por qué quedó obsoleta y un registro de cambios limitado. |
| **Actualiza** | Cuando una Página se marca como obsoleta, reconstruye las Páginas mantenidas automáticamente que cumplen los requisitos a partir de la evidencia actual. |
| **Revisa** | Durante la actualización automática, propone cambios en las Páginas que hayas editado, en lugar de reescribirlas sin avisar. |

Por ejemplo, importa un documento de diseño y guarda en Codex una decisión de depuración. Wenlan puede reunir ambos en una Página con citas a las dos fuentes. Cuando la Página se actualice automáticamente, se reconstruirá a partir de los materiales que la sustentan; si la has editado, la propuesta de cambio quedará pendiente de revisión.

**Alcance de la revisión:** es una política de actualización, no una barrera de seguridad para tus archivos. Las ediciones directas en archivos y las realizadas mediante la API local de edición manual no pasan por esta cola. Una regeneración forzada y explícita también puede reemplazar una Página editada; la aplicación de escritorio pide confirmación antes de hacerlo.

<a id="local-markdown"></a>

### Markdown local que funciona con Obsidian

Tu síntesis duradera permanece en archivos ordinarios en lugar de un formato de editor propietario:

- **Archivos planos:** Las Páginas y notas de sesión permanecen como Markdown en `~/.wenlan/`.
- **Historial inspeccionable:** cada Página conserva sus revisiones (visibles en la aplicación o con `get_page_revisions` por MCP); como todo son archivos planos, puedes poner `~/.wenlan` bajo tu propio control de versiones.
- **Coexistencia con Obsidian:** Wenlan lee un vault existente como una fuente. Crea un enlace simbólico de `~/.wenlan/pages/` hacia el vault o exporta una Página desde la aplicación de escritorio; tus ediciones siguen siendo propiedad humana, y las actualizaciones posteriores de la máquina se convierten en revisiones revisables.

Las Páginas se leen sin la aplicación:

```text
$ wenlan pages "embedding work" --print
```
