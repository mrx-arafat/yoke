async (page) => {
  // Run with the isolated yoke-w5 Playwright session after injecting local axe-core.
  const reports=[];
  for(const width of [1440,390]) {
    await page.setViewportSize({width,height:width===390?844:1000});
    for(const theme of ["light","dark"]) {
      await page.locator("#theme").selectOption(theme);
      for(const name of ["decisions","sessions","plans","memory","trust","usage"]) {
        await page.evaluate(()=>{
          window.__navigationMs=null;
          document.addEventListener("click",()=>{
            const start=performance.now();
            const observer=new MutationObserver(()=>{
              if(document.getElementById("view").getAttribute("aria-busy")!=="true"){
                observer.disconnect();requestAnimationFrame(()=>{window.__navigationMs=performance.now()-start;});
              }
            });
            observer.observe(document.getElementById("view"),{attributes:true,childList:true});
          },{once:true,capture:true});
        });
        await page.locator(`nav[aria-label=Workspace] a[href^="/${name}"]`).click();
        await page.waitForFunction(name=>document.querySelector("main")?.dataset.page===name && document.getElementById("view").getAttribute("aria-busy")!=="true",name);
        const dismiss=page.getByRole("button",{name:"Dismiss notification"});
        if(await dismiss.isVisible())await dismiss.click();
        const snapshot=await page.locator("main").ariaSnapshot();
        const result=await page.evaluate(async()=>{
          const scan=await axe.run(document,{runOnly:{type:"tag",values:["wcag2a","wcag2aa","wcag21aa","wcag22aa"]}});
          return {violations:scan.violations.map(v=>({id:v.id,impact:v.impact,targets:v.nodes.map(n=>n.target)})),overflow:document.documentElement.scrollWidth-innerWidth,navigationMs:window.__navigationMs};
        });
        const screenshot=`artifacts/${name}-${theme}-${width}.png`;
        await page.screenshot({path:screenshot});
        reports.push({name,theme,width,screenshot,heading:snapshot.split("\n").find(v=>v.includes("heading")),...result});
      }
    }
  }
  return reports;
}
