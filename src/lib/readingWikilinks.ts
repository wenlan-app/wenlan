// SPDX-License-Identifier: AGPL-3.0-only
export interface ResolvedReadingWikilink { label: string; href?: string }
interface MarkdownNode { type: string; value?: string; url?: string; children?: MarkdownNode[]; position?: {start:{offset?:number};end:{offset?:number}} }
/** Parse Markdown first, so code, existing links and escaped brackets stay literal. */
export function remarkReadingWikilinks(options: {source:string;resolve(inner:string):ResolvedReadingWikilink|null}) {
  const visit=(node:MarkdownNode)=>{
    if(!node.children || ["link","linkReference","image","imageReference"].includes(node.type))return;
    node.children=node.children.flatMap((child):MarkdownNode[]=>{
      if(child.type!=="text" || !child.value){visit(child);return [child];}
      const raw=options.source.slice(child.position?.start.offset,child.position?.end.offset);
      if(/\\\[/.test(raw))return [child];
      const result:MarkdownNode[]=[];let cursor=0;
      for(const match of child.value.matchAll(/\[\[([^\]\n]+)\]\]/g)){
        if(match.index>0 && child.value[match.index-1]==="!")continue;
        const target=options.resolve(match[1]);if(!target)continue;
        if(match.index>cursor)result.push({type:"text",value:child.value.slice(cursor,match.index)});
        result.push(target.href?{type:"link",url:target.href,children:[{type:"text",value:target.label}]}:{type:"text",value:target.label});
        cursor=match.index+match[0].length;
      }
      if(!cursor)return [child];
      if(cursor<child.value.length)result.push({type:"text",value:child.value.slice(cursor)});
      return result;
    });
  };return (tree:unknown)=>visit(tree as MarkdownNode);
}
